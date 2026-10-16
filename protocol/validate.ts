import {validateWriteModel,validateWriteIntent,writePayloadTypes} from './write_validate';
import {commandFields, editorOperations, limits, type RemoteRequest} from './generated/contract';
import {modelFields,modelEnums} from './generated/models';
const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
function object(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw Error('Expected object');
  return value as Record<string, unknown>;
}
function integer(value: unknown, minimum: number = 0, maximum: number = limits.safeInteger) {
  if (typeof value !== 'number' || !Number.isSafeInteger(value) || value < minimum || value > maximum) throw Error('Invalid integer');
}
function fields(value: Record<string,unknown>, allowed: string[], required: string[]) {
  if (Object.keys(value).some(key => !allowed.includes(key)) || required.some(key => !(key in value))) throw Error('Invalid fields');
}
function tree(value: unknown, depth = 0): void {
  if (depth > 64) throw Error('Too deeply nested');
  if (typeof value === 'number' && (!Number.isFinite(value) || Math.abs(value) > limits.safeInteger)) throw Error('Unsafe number');
  if (Array.isArray(value)) value.forEach(child => tree(child, depth + 1));
  else if (value && typeof value === 'object') for (const [key, child] of Object.entries(value)) { if (key.length > 256) throw Error('Oversized key'); tree(child, depth + 1); }
}
function typed(value: unknown, type: string): void {
  if (type === 'UUID') { if (typeof value !== 'string' || !uuid.test(value)) throw Error('Invalid UUID'); }
  else if (type === 'Int') integer(value, -limits.safeInteger);
  else if (type === 'UInt32') integer(value, 0, 4294967295);
  else if (type === 'Float' || type === 'Double') { if (typeof value !== 'number' || !Number.isFinite(value)) throw Error('Invalid number'); }
  else if (type === 'Bool') { if (typeof value !== 'boolean') throw Error('Invalid boolean'); }
  else if (type === 'String') { if (typeof value !== 'string') throw Error('Invalid string'); }
  else if (type === 'Data') { if (typeof value !== 'string' || !/^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/.test(value)) throw Error('Invalid base64'); }
  else if (type.startsWith('[') && !type.includes(':')) { if (!Array.isArray(value)) throw Error('Invalid array'); value.forEach(item => typed(item,type.slice(1,-1))); }
  else validateModel(value,type);
}
/** Wire shape validation is not permission or domain validation. Server owns both. */
export function validateRequest(value: unknown): RemoteRequest {
  tree(value); const request = object(value);
  fields(request,['version','kind','id','clientID','expectedRevision','expectedTrackRevisions','command'],['version','kind','id','clientID','command']);
  if (request.version !== 3 || request.kind !== 'request') throw Error('Unsupported protocol');
  typed(request.id,'UUID'); typed(request.clientID,'UUID');
  if ('expectedRevision' in request) integer(request.expectedRevision);
  for (const [id, revision] of Object.entries(object(request.expectedTrackRevisions ?? {}))) { typed(id,'UUID'); integer(revision); }
  const command = object(request.command);
  if (typeof command.type !== 'string' || !Object.hasOwn(commandFields, command.type)) throw Error('Unknown command');
  const descriptor = commandFields[command.type as keyof typeof commandFields];
  fields(command,['type',...descriptor.map(field => field.name)],['type',...descriptor.filter(field => !field.optional).map(field => field.name)]);
  for (const field of descriptor) { const value = command[field.name]; if (value !== undefined && value !== null) typed(value,field.type); else if (!field.optional) throw Error('Missing field'); }
  for (const key of ['frame','start','end','offset','bytes','channel']) if (command[key] !== undefined && command[key] !== null) integer(command[key]);
  if (command.type === 'editor') {
    const intent = object(command.intent);
    if (typeof intent.operation !== 'string' || !(editorOperations as readonly string[]).includes(intent.operation)) throw Error('Unknown operation');
    const context = object(intent.context); for (const key of ['cursorFrame','rangeStart','rangeEnd']) if (context[key] != null) integer(context[key]);
    validateWriteIntent(intent);
    const payloadTypes: Record<string,string> = {
      ...writePayloadTypes,
      'designer.preview':'SoundDesignPattern','designer.configure':'SoundDesignPattern','designer.noteOn':'SoundDesignPattern','designer.midiConnect':'SoundDesignPattern','designer.commit':'SoundDesignPattern',
      'drums.preview':'DrumPattern','drums.commit':'DrumPattern','drums.export':'DrumPattern','effects.preview':'[String: Float]','effects.apply':'[String: Float]',
      'pitch.manualApply':'[String: Float]','client.features':'[String]','master.applyProposal':'[RealtimeEffect]','meters.subscribe':'[UUID]','project.export':'[UUID]'
    };
    if (intent.payload != null && payloadTypes[intent.operation]) {
      const binary = atob(intent.payload as string); const text = new TextDecoder('utf-8',{fatal:true}).decode(Uint8Array.from(binary,c=>c.charCodeAt(0)));
      validateModel(JSON.parse(text),payloadTypes[intent.operation]!);
    }
  }
  if(command.type==='editor' && writePayloadTypes[(command.intent as Record<string,unknown>).operation as string]) {
    const intent=command.intent as Record<string,unknown>;const payload=JSON.parse(new TextDecoder().decode(Uint8Array.from(atob(intent.payload as string),c=>c.charCodeAt(0)))) as Record<string,unknown>;
    const versions=Object.keys(request.expectedTrackRevisions as object??{});
    if(request.expectedRevision!=null || (intent.operation==='automation.writeBegin' ? versions.length!==1 || versions[0]!.toUpperCase()!==(payload.trackID as string).toUpperCase() : versions.length!==0)) throw Error('Invalid Write resource fence');
  }
  if (command.type === 'beginUpload') integer(command.bytes,1,limits.maxUploadBytes);
  if (command.type === 'uploadChunk' && typeof command.data === 'string' && atob(command.data).length > limits.maxUploadChunkBytes) throw Error('Oversized chunk');
  return request as unknown as RemoteRequest;
}

/** Validate the same nested record grammar used by the native v3 adapter. */
export function validateModel(value: unknown, type: string, depth = 0): void {
  if (depth > 64) throw Error('Too deeply nested');
  if (type === 'Int') { integer(value,-limits.safeInteger); return; }
  if (type === 'UInt32') { integer(value,0,4294967295); return; }
  if (type === 'UInt64') { integer(value); return; }
  if (type === 'UInt64String') {
    if (typeof value !== 'string' || !/^(0|[1-9][0-9]{0,19})$/.test(value) || BigInt(value) > 18446744073709551615n) throw Error('Invalid UInt64 string');
    return;
  }
  if (type === 'Float' || type === 'Double' || type === 'Date') {
    if (typeof value !== 'number' || !Number.isFinite(value) || Math.abs(value) > limits.safeInteger) throw Error('Invalid number'); return;
  }
  if (['UUID','String','Data','Bool'].includes(type)) { typed(value,type); return; }
  if (type === 'Range<Int>') {
    if (!Array.isArray(value) || value.length !== 2) throw Error('Invalid range');
    value.forEach(item => integer(item,-limits.safeInteger)); if (value[0] > value[1]) throw Error('Reversed range'); return;
  }
  if (type.startsWith('[')) {
    const inner = type.slice(1,-1); let nesting = 0; let delimiter = -1;
    for (let i = 0; i < inner.length; i++) {
      const c = inner[i]; if (c === '[' || c === '<') nesting++; else if (c === ']' || c === '>') nesting--;
      else if (c === ':' && nesting === 0) { delimiter = i; break; }
    }
    if (delimiter < 0) {
      if (!Array.isArray(value) || value.length > 100000) throw Error('Invalid array'); value.forEach(item => validateModel(item,inner,depth+1));
    } else {
      const values = object(value); if (Object.keys(values).length > 1024) throw Error('Too many entries');
      const keyType = inner.slice(0,delimiter).trim(), valueType = inner.slice(delimiter+1).trim(); const ids = new Set<string>();
      for (const [key,item] of Object.entries(values)) {
        validateModel(key,keyType,depth+1); if (keyType === 'UUID') { const id = key.toLowerCase(); if (ids.has(id)) throw Error('Duplicate UUID'); ids.add(id); }
        validateModel(item,valueType,depth+1);
      }
    }
    return;
  }
  const enumerations = modelEnums as Record<string,readonly string[]>;
  if (Object.hasOwn(enumerations,type)) { if (typeof value !== 'string' || !enumerations[type]!.includes(value)) throw Error('Invalid enum'); return; }
  const records = modelFields as Record<string,readonly {name:string;type:string;optional:boolean;required:boolean}[]>;
  if (!Object.hasOwn(records,type)) throw Error('Unknown model'); const record = object(value); const descriptor = records[type]!;
  fields(record,descriptor.map(field=>field.name),descriptor.filter(field=>field.required).map(field=>field.name));
  for (const field of descriptor) {
    if (record[field.name] !== undefined && record[field.name] !== null) validateModel(record[field.name],field.type,depth+1);
    else if (field.required || (record[field.name] === null && !field.optional)) throw Error('Missing required model field');
  }
  if (type === 'Track' || type === 'RemoteTrackControls') {
    if (record.gain!=null && ((record.gain as number)<0 || (record.gain as number)>4)) throw Error('Invalid track gain');
    if (record.pan!=null && ((record.pan as number)< -1 || (record.pan as number)>1)) throw Error('Invalid track pan');
  }
  if (type === 'RemoteTrackControlPatch') {
    if (['gain','pan','muted','solo','armed'].every(key=>record[key]==null)) throw Error('Empty scalar patch');
    if (record.gain!=null && ((record.gain as number)<0 || (record.gain as number)>4)) throw Error('Invalid patch gain');
    if (record.pan!=null && ((record.pan as number)< -1 || (record.pan as number)>1)) throw Error('Invalid patch pan');
  }
  if (type === 'RemoteSessionDelta' && record.controlPatches!=null) {
    const patches=record.controlPatches as Record<string,unknown>[];
    if(patches.length>1024 || new Set(patches.map(p=>(p.trackID as string).toUpperCase())).size!==patches.length) throw Error('Repeated/oversized scalar patch set');
  }
  if(type==='RemoteEditorIntent') { const payloadType=validateWriteIntent(record); if(payloadType) {const binary=atob(record.payload as string);validateModel(JSON.parse(new TextDecoder('utf-8',{fatal:true}).decode(Uint8Array.from(binary,c=>c.charCodeAt(0)))),payloadType);} }
  if (type.startsWith('RemoteAutomationWrite')) validateWriteModel(record,type);
  if (type === 'Session' && record.version !== undefined) integer(record.version,1,11);
  if (type === 'MusicalTiming') {
    if (typeof record.tempo !== 'number' || record.tempo < 20 || record.tempo > 300) throw Error('Invalid tempo'); integer(record.beatsPerBar,1,16);
    if (![2,4,8,16].includes(record.beatUnit as number)) throw Error('Invalid meter');
  }
}
