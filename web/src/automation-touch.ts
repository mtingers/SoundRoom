import {AutomationWriteController,sameWriteTarget} from './automation-write.js';
import type {RemoteAutomationWriteTarget as WriteTarget} from './generated/models.js';
const key=(trackID:string,target:WriteTarget)=>`${trackID.toUpperCase()}:${target.kind}:${target.effectID?.toUpperCase()??''}:${target.parameterID??''}`;
export interface TouchGesture {readonly ended:boolean;offer(value:number):boolean;end(cancel?:boolean):Promise<void>;}
/** Private per-target Touch arming. A control never falls back to a static edit after a declined live Begin. */
export class AutomationTouchBindings {
 private targets=new Set<string>();
 constructor(readonly controller:AutomationWriteController,private report:(message:string)=>void){}
 armed(trackID:string,target:WriteTarget){return this.targets.has(key(trackID,target));}
 supported(trackID:string,target:WriteTarget){return this.controller.supported(trackID,target);}
 arm(trackID:string,target:WriteTarget,enabled:boolean){const identity=key(trackID,target);if(!enabled){this.targets.delete(identity);return true;}if(!this.supported(trackID,target))return false;if(this.targets.size>=128&&!this.targets.has(identity))return false;this.targets.add(identity);return true;}
 reset(){this.targets.clear();}
 start(trackID:string,target:WriteTarget):TouchGesture|undefined {
  if(!this.armed(trackID,target))return undefined;
  const unsupported=!this.supported(trackID,target),before=this.controller.current?.gestureID,pending=this.controller.begin(trackID,target),current=this.controller.current,gestureID=current?.gestureID!==before&&current?.phase==='beginning'?current.gestureID:undefined;
  let ended=false;const owns=()=>!!gestureID&&this.controller.current?.gestureID===gestureID&&this.controller.current.trackID===trackID&&sameWriteTarget(this.controller.current.target,target);
  void pending.then(accepted=>{if(gestureID&&this.controller.current?.gestureID!==gestureID)return;if(!accepted)this.report(gestureID?this.controller.current?.reason??'Touch automation could not start.':unsupported?'Touch is unavailable for this server, role, or parameter. Choose Read to edit normally.':'Play the mix without a loop, or finish the current Touch gesture, before recording this control.');});
  return {get ended(){return ended;},offer:value=>!ended&&owns()&&this.controller.offer(value),end:async(cancel=false)=>{if(ended)return;ended=true;if(owns())await this.controller.end(cancel);}};
 }
}

/** Pointer release and keyboard key-up delimit one human gesture, including range change events arriving after pointer-up. */
export class TouchControlBinding {
 private gesture?:TouchGesture;private held?:'pointer'|'keyboard';private releasedPointer=false;
 constructor(private bindings:AutomationTouchBindings,private trackID:string,private target:WriteTarget){}
 get active(){return !!this.gesture&&!this.gesture.ended;}
 matches(trackID:string,target:WriteTarget){return this.trackID===trackID&&sameWriteTarget(this.target,target);}
 private start(){this.gesture=this.bindings.start(this.trackID,this.target);}
 pointerDown(){this.releasedPointer=false;this.held='pointer';this.start();}
 keyDown(key:string,repeat=false){if(!['ArrowLeft','ArrowRight','ArrowUp','ArrowDown','Home','End','PageUp','PageDown'].includes(key))return;if(!repeat){this.releasedPointer=false;this.held='keyboard';this.start();}}
 input(value:number){this.releasedPointer=false;if(!this.gesture||this.gesture.ended)this.start();if(!this.gesture)return false;this.gesture.offer(value);return true;}
 change(value:number){if(this.releasedPointer){this.releasedPointer=false;return !!this.gesture;}const handled=this.input(value);if(!this.held)void this.gesture?.end();return handled;}
 pointerUp(value:number){this.gesture?.offer(value);void this.gesture?.end();this.held=undefined;this.releasedPointer=true;}
 keyUp(value:number){if(this.held!=='keyboard')return;this.gesture?.offer(value);void this.gesture?.end();this.held=undefined;}
 blur(){void this.gesture?.end();this.held=undefined;}
 cancel(){void this.gesture?.end(true);this.held=undefined;}
}
