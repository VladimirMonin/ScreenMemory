import test from 'node:test';
import assert from 'node:assert/strict';
import { randomUUID, randomBytes, createHash } from 'node:crypto';
import { spawn, execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { rm } from 'node:fs/promises';
import { once } from 'node:events';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { SdkAdapter } from '../src/sdk-adapter.ts';
import { endpoint } from './synthetic-endpoint.mjs';

const plan = () => {const id=randomUUID();return {intent_id:id,identifier:`sm-${id}`,model_key:'synthetic/vision.gguf',context_length:8192,eval_batch_size:256,flash_attention:true,offload_kv_cache_to_gpu:true,k_cache_type:'F16',v_cache_type:'Q8_0',ttl_seconds:900};};
const png=Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVQIHWP4z8DwHwAFgAI/ScLttAAAAABJRU5ErkJggg==','base64');
const schema={type:'object',additionalProperties:false,required:['title'],properties:{title:{type:'string',maxLength:160}}};
const vision=reference=>({instance_reference:reference,prompt:'Describe synthetic pixels only',schema,max_tokens:4000,temperature:0.2,reasoning:'backend_default'});

test('actual published SDK inventory/load/config/vision: immutable identity, local mapping, max4000', {timeout:10000}, async()=>{
  const own=await endpoint(); const adapter=new SdkAdapter(own.url,'sk-lm-syntest1:01234567890123456789');
  try {
    assert.equal((await adapter.inventory()).downloaded.length,1);
    const loaded=await adapter.load(plan(),new AbortController().signal);
    assert.equal(loaded.configuration,'reported_applied');
    assert.deepEqual(loaded.native_ttl,{requested_seconds:900,verification:'not_verified'});
    assert.equal(own.state.token,'01234567890123456789');
    const result=await adapter.vision(vision(loaded.instance_reference),png.toString('base64'),new AbortController().signal);
    assert.equal(result.complete,true);assert.equal(result.value.title,'Synthetic only');assert.equal(result.usage.reasoning_tokens,null);
    const calls=own.state.messages; const load=calls.find(m=>m.endpoint==='loadModel');
    assert.equal(load.creationParameter.deviceIdentifier,null);assert.equal(load.creationParameter.ttlMs,900000);
    const prediction=calls.find(m=>m.endpoint==='predict');
    assert.deepEqual(prediction.creationParameter.modelSpecifier,{type:'instanceReference',instanceReference:loaded.instance_reference});
    const fields=prediction.creationParameter.predictionConfigStack.layers.flatMap(l=>l.config.fields);
    assert.ok(fields.some(f=>f.key.endsWith('maxPredictedTokens') && f.value.checked===true && f.value.value===4000));
    assert.ok(prediction.creationParameter.history.messages[0].content.some(p=>p.type==='file' && p.identifier==='synthetic-image'));
    assert.ok(!calls.some(m=>['getOrLoad','unloadModel'].includes(m.endpoint)));
  } finally { await adapter.close();await own.close(); }
});
test('collision, foreign/replaced reference, unsupported reasoning/unload have no unsafe SDK calls', {timeout:10000}, async()=>{
  const own=await endpoint();const adapter=new SdkAdapter(own.url,'');
  try {
    const p=plan(); const loaded=await adapter.load(p,new AbortController().signal);
    await assert.rejects(adapter.load(p,new AbortController().signal),{code:'IDENTIFIER_COLLISION'});
    await assert.rejects(adapter.vision({...vision(loaded.instance_reference),reasoning:'budget100'},png.toString('base64'),new AbortController().signal),{code:'REASONING_UNSUPPORTED'});
    await assert.rejects(adapter.unload(loaded.instance_reference),{code:'UNLOAD_IDENTITY_ATOMICITY_UNSUPPORTED'});
    own.state.loaded[0].instanceReference='replacement';
    await assert.rejects(adapter.observe(loaded.instance_reference));
    assert.equal(own.state.messages.filter(m=>m.endpoint==='loadModel').length,1);
    assert.equal(own.state.messages.filter(m=>m.endpoint==='unloadModel'||m.endpoint==='predict').length,0);
  } finally {await adapter.close();await own.close();}
});
test('SDK actual prediction cancellation settles, truncated response never publishes structured value', {timeout:10000}, async()=>{
  const own=await endpoint();const adapter=new SdkAdapter(own.url,'');
  try {
    const loaded=await adapter.load(plan(),new AbortController().signal);
    own.state.mode='invalid_json';await assert.rejects(adapter.vision(vision(loaded.instance_reference),png.toString('base64'),new AbortController().signal),{code:'STRUCTURED_INVALID'});
    own.state.mode='truncate';const truncated=await adapter.vision(vision(loaded.instance_reference),png.toString('base64'),new AbortController().signal);
    assert.equal(truncated.complete,false);assert.equal(truncated.value,null);
    own.state.mode='hang';const cancel=new AbortController();const result=adapter.vision(vision(loaded.instance_reference),png.toString('base64'),cancel.signal);
    const timer=setInterval(()=>{if(own.state.messages.filter(m=>m.endpoint==='predict').length===3)cancel.abort();},5);
    try {const outcome=await result;assert.equal(outcome.stop_reason,'userStopped');assert.equal(outcome.complete,false);assert.ok(own.state.cancelled>0);}finally{clearInterval(timer);}
  } finally {await adapter.close();await own.close();}
});

export function childBridge(url, entry=resolve(fileURLToPath(new URL('../src/bridge.mjs',import.meta.url))), node=process.execPath) {
  const child=spawn(node,['--no-global-search-paths','--no-addons',entry],{env:{},stdio:['pipe','pipe','pipe'],windowsHide:true});
  const auth=randomBytes(32).toString('hex'),generation=randomUUID();let counter=0;let buffer=Buffer.alloc(0),stderr='';const waiters=new Map();
  child.stderr.on('data',b=>stderr+=b.toString());
  child.stdout.on('data',b=>{
    buffer=Buffer.concat([buffer,b]);while(buffer.length>=4&&buffer.length>=4+buffer.readUInt32LE()){
      const n=buffer.readUInt32LE(),value=JSON.parse(buffer.subarray(4,n+4));buffer=buffer.subarray(n+4);
      const waiter=waiters.get(value.id);if(waiter){waiters.delete(value.id);waiter(value);}
    }
  });
  function send(raw,image=Buffer.alloc(0)) {
    const promise=new Promise(resolve=>waiters.set(raw.id??'0',resolve));const bytes=Buffer.from(JSON.stringify(raw));const header=Buffer.alloc(4);header.writeUInt32LE(bytes.length);child.stdin.write(Buffer.concat([header,bytes,image]));return promise;
  }
  return {child,auth,generation,stderr:()=>stderr,
    init:()=>send({protocol:'sm-sdk/1.1',auth,generation,endpoint:url,api_token:'sk-lm-syntest1:01234567890123456789'}),
    request:(method,params={},image=Buffer.alloc(0),deadline_ms=3000)=>{const id=String(++counter);return {id,result:send({id,auth,generation,method,params,deadline_ms,image_length:image.length},image)};},
    send, stop:async()=>{if(child.exitCode===null){child.kill();await once(child,'exit');}}
  };
}
test('private child authenticated framing actual SDK and safe diagnostics; Off/image rejection are explicit', {timeout:15000}, async()=>{
  const own=await endpoint(),bridge=childBridge(own.url);
  try{
    const hello=await bridge.init();assert.equal(hello.result.sdk,'2.0.0');
    const loaded=await bridge.request('load',plan()).result;assert.equal(loaded.ok,true);
    const p={...vision(loaded.result.instance_reference),width:1,height:1,max_side:1024,sha256:createHash('sha256').update(png).digest('hex')};
    const result=await bridge.request('vision',p,png).result;assert.equal(result.result.complete,true);
    const invalid=await bridge.request('vision',{...p,sha256:'0'.repeat(64)},png).result;assert.equal(invalid.error.code,'IMAGE_INVALID');
    const unknown=await bridge.request('status',{target:'999'}).result;assert.equal(unknown.result.state,'unknown');
    assert.ok(!bridge.stderr().includes('01234567890123456789'));assert.equal(bridge.stderr(),'');
  } finally {await bridge.stop();await own.close();}
});
test('authentication fails closed before SDK connection', {timeout:10000}, async()=>{
  const own=await endpoint(),bridge=childBridge(own.url);
  try{
    await bridge.init();const exit=once(bridge.child,'exit');
    bridge.send({id:'1',auth:'0'.repeat(64),generation:bridge.generation,method:'inventory',params:{},deadline_ms:100,image_length:0});
    assert.equal((await exit)[0],7);assert.equal(own.state.messages.length,0);assert.equal(bridge.stderr(),'BRIDGE_PROTOCOL_REFUSED\n');
    const oversized=childBridge(own.url);
    try {
      const end=once(oversized.child,'exit');const length=Buffer.alloc(4);length.writeUInt32LE(4*1024*1024+1);
      oversized.child.stdin.write(length);assert.equal((await end)[0],7);
      assert.equal(own.state.messages.length,0);
    }finally{await oversized.stop();}
  } finally {await bridge.stop();await own.close();}
});

test('control cancellation stays responsive; ignored backend cancellation fences mutations', {timeout:10000}, async()=>{
  const own=await endpoint(),bridge=childBridge(own.url);
  try{
    await bridge.init();const loaded=await bridge.request('load',plan()).result;
    const p={...vision(loaded.result.instance_reference),width:1,height:1,max_side:1024,sha256:createHash('sha256').update(png).digest('hex')};
    own.state.mode='hang';const prediction=bridge.request('vision',p,png);
    while(!own.state.messages.some(m=>m.endpoint==='predict'))await new Promise(r=>setTimeout(r,5));
    const ack=await bridge.request('cancel',{target:prediction.id}).result;assert.equal(ack.ok,true);
    const cancelled=await prediction.result;assert.equal(cancelled.result.stop_reason,'userStopped');assert.equal(cancelled.result.value,null);
    own.state.mode='ignore_cancel';const uncertain=await bridge.request('vision',p,png,10).result;
    assert.equal(uncertain.error.settlement,'uncertain');
    const refused=await bridge.request('load',plan()).result;assert.equal(refused.error.code,'RECONCILIATION_REQUIRED');
    assert.equal(own.state.messages.filter(m=>m.endpoint==='loadModel').length,1);
  }finally{await bridge.stop();await own.close();}
});
test('offline staged clone executes bundled Node/SDK with empty environment and no dev PATH', {timeout:15000}, async()=>{
  const own=await endpoint();const stage=resolve(fileURLToPath(new URL('../../../.pi/temporary/',import.meta.url)),randomUUID());
  let bridge;
  try{
    await promisify(execFile)(process.execPath,[fileURLToPath(new URL('../scripts/stage.mjs',import.meta.url)),stage],{env:{},timeout:10000});
    bridge=childBridge(own.url,resolve(stage,'src/bridge.mjs'),resolve(stage,'node.exe'));
    assert.equal((await bridge.init()).result.sdk,'2.0.0');
    const loaded=await bridge.request('load',plan()).result;assert.equal(loaded.ok,true);
    const p={...vision(loaded.result.instance_reference),width:1,height:1,max_side:1024,sha256:createHash('sha256').update(png).digest('hex')};
    assert.equal((await bridge.request('vision',p,png).result).result.complete,true);
    assert.equal(bridge.stderr(),'');
  }finally{if(bridge)await bridge.stop();await own.close();await rm(stage,{recursive:true,force:false});}
});
