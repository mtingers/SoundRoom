"""Stored-field grammar for the migration's supported Codable model subset.
Fail closed when a new stored-field spelling cannot be represented; do not silently
invent a project schema. Domain constraints supplement these structural descriptors.
"""
import re

def split_top(text, delimiter=','):
    out=[]; start=0; depth=0; quoted=False; escaped=False
    for i,c in enumerate(text):
        if quoted:
            if escaped: escaped=False
            elif c=='\\': escaped=True
            elif c=='"': quoted=False
        elif c=='"': quoted=True
        elif c in '([<': depth+=1
        elif c in ')]>': depth-=1
        elif c==delimiter and depth==0: out.append(text[start:i].strip()); start=i+1
    out.append(text[start:].strip()); return out

def body_at(text,start):
    depth=1; end=start
    while end<len(text) and depth:
        depth+=(text[end]=='{')-(text[end]=='}'); end+=1
    return text[start:end-1]

def infer(expr,name):
    expr=expr.strip()
    if expr in ('true','false'): return 'Bool'
    if expr.startswith('"'): return 'String'
    if re.fullmatch(r'-?[0-9_]+',expr): return 'Int'
    if re.fullmatch(r'-?[0-9_]+\.[0-9_]+',expr): return 'Double'
    if expr.startswith('UUID('): return 'UUID'
    if expr=='Date()': return 'Date'
    if expr.startswith('Array('): return '[Int]'
    if expr.startswith('[') and expr.endswith(']'):
        values=split_top(expr[1:-1]); return '['+infer(values[0],name)+']'
    if expr=='Session.defaultMasterTrack': return 'Track'
    if expr.startswith('RemoteProtocol.'): return 'Int'
    if expr.endswith('.keys.sorted()') or expr=='RemoteV3Descriptor.editorOperations.sorted()': return '[String]'
    match=re.match(r'^(\w+)(?:\.|\()',expr)
    if match: return match[1]
    raise ValueError('Cannot infer '+name+' from '+expr)

def enum_declarations(paths):
    """Return native spellings as well as wire values, including escaped identifiers."""
    declarations={}
    for path in paths:
        text=path.read_text()
        for match in re.finditer(r'public enum (\w+):([^\n{]*String[^\n{]*Codable[^\n{]*)\{',text):
            entries=[]; depth=0
            for line in body_at(text,match.end()).splitlines():
                case=re.match(r'\s*case (.*)',line) if depth==0 else None
                depth+=line.count('{')-line.count('}')
                if not case: continue
                for item in split_top(case[1]):
                    value=re.fullmatch(r'(`?\w+`?)(?:\s*=\s*"([^"]*)")?',item.strip())
                    if not value: raise ValueError('Unsupported enum case '+match[1]+'.'+item)
                    entries.append({'identifier':value[1], 'value':value[2] if value[2] is not None else value[1].strip('`')})
            scopes=[]
            for container in re.finditer(r'public (?:struct|enum) (\w+)[^\n{]*\{',text[:match.start()]):
                if container.end()+len(body_at(text,container.end()))>=match.end(): scopes.append(container[1])
            if match[1] in declarations: raise ValueError('Ambiguous enum model '+match[1])
            declarations[match[1]]={'entries':entries,'iterable':'CaseIterable' in match[2], 'nativeName':'.'.join(scopes+[match[1]])}
    return declarations

def extract(paths):
    records={}; enums={name:[case['value'] for case in declaration['entries']] for name,declaration in enum_declarations(paths).items()}
    for path in paths:
        text=path.read_text()
        for match in re.finditer(r'public struct (\w+):[^\n{]*Codable[^\n{]*\{',text):
            body=body_at(text,match.end()); fields=[]
            custom='init(from decoder:' in body
            required=set(re.findall(r'\.decode\([^\n;]*?forKey:\s*\.(\w+)',body)) if custom else None
            for line in body.splitlines():
                m=re.match(r'\s*public (?:var|let) (.*)',line)
                if not m: continue
                declaration=m[1].split(' //',1)[0]
                if '{' in declaration or '\\' in declaration: continue
                items=split_top(declaration); pending=[]
                for item in items:
                    prop=re.match(r'^(\w+)(?:\s*:\s*([^=]+))?(?:\s*=\s*(.*))?$',item)
                    if not prop: raise ValueError('Unsupported field '+match[1]+'.'+item)
                    name,typ,default=prop.groups()
                    if not typ and default is None: pending.append(name); continue
                    typ=typ.strip() if typ else infer(default,name)
                    for field_name in pending+[name]:
                        fields.append({'name':field_name,'type':typ.rstrip('?'),'optional':typ.endswith('?'),'required':field_name in required if custom else not typ.endswith('?')})
                    pending=[]
                if pending: raise ValueError('Missing field type '+declaration)
            coding_match=re.search(r'enum CodingKeys:[^\n{]*CodingKey[^\n{]*\{',body)
            if coding_match:
                coding_body=body_at(body,coding_match.end()); mapping={}
                for line in coding_body.splitlines():
                    case=re.match(r'\s*case (.*)',line)
                    if not case: continue
                    for item in split_top(case[1]):
                        key=re.fullmatch(r'(`?\w+`?)(?:\s*=\s*"([^"]*)")?',item.strip())
                        if not key: raise ValueError('Unsupported coding key '+match[1]+'.'+item)
                        name=key[1].strip('`'); mapping[name]=key[2] if key[2] is not None else name
                fields=[field for field in fields if field['name'] in mapping]
                for field in fields: field['name']=mapping[field['name']]
            records[match[1]]={'source':str(path),'fields':fields}
    return records,enums
