import { WebSocketServer } from 'ws';
import { once } from 'node:events';
import { createInterface } from 'node:readline';

export const modelInfo = {
  type:'llm', modelKey:'synthetic/vision.gguf', format:'gguf', displayName:'Synthetic',
  publisher:'synthetic', path:'synthetic/vision.gguf', sizeBytes:1,
  indexedModelIdentifier:'synthetic', deviceIdentifier:null, vision:true,
  trainedForToolUse:false, maxContextLength:8192, contextLength:8192,
  identifier:'', instanceReference:'synthetic-reference-immutable', ttlMs:900000, lastUsedTime:null
};
export async function endpoint() {
  const server = new WebSocketServer({host:'127.0.0.1',port:0}); await once(server,'listening');
  const state = { messages:[], loaded:[], config:{fields:[]}, mode:'complete', token:undefined, cancelled:0 };
  server.on('connection',(ws,request) => {
    let authenticated=false; let prediction;
    const send=value=>{ if(ws.readyState===ws.OPEN) ws.send(JSON.stringify(value)); };
    const packet=(id,message)=>send({type:'channelSend',channelId:id,message});
    const settle=(id,reason)=>{
      if (reason==='eosFound') packet(id,{type:'fragment',fragment:{content:state.mode==='invalid_json'?'not-json':'{"title":"Synthetic only"}',tokensCount:3,containsDrafted:false,reasoningType:'none',isStructural:false}});
      packet(id,{type:'success',stats:{stopReason:reason,promptTokensCount:8,predictedTokensCount:3,totalTokensCount:11},modelInfo:state.loaded[0],loadModelConfig:state.config,predictionConfig:{fields:[]}});
      send({type:'channelClose',channelId:id});
    };
    ws.on('message',data=>{
      const message=JSON.parse(data.toString());
      if (!authenticated) { authenticated=true; state.token=message.clientPasskey; send({success:true}); return; }
      state.messages.push({namespace:request.url,...message});
      if (message.type==='keepAlive') { send({type:'keepAliveAck'}); return; }
      if (message.type==='rpcCall') {
        let result;
        switch(message.endpoint) {
          case 'listDownloadedModels': result=[modelInfo]; break;
          case 'listLoaded': result=state.loaded; break;
          case 'getModelInfo': result=state.loaded.find(m=>m.instanceReference===message.parameter.specifier.instanceReference); break;
          case 'getLoadConfig': result=state.config; break;
          case 'getBasePredictionConfig': result={fields:[]}; break;
          case 'uploadFileBase64': result={identifier:'synthetic-image',fileType:'image',sizeBytes:Buffer.from(message.parameter.contentBase64,'base64').length}; break;
          default: throw new Error('UNEXPECTED_SYNTHETIC_RPC');
        }
        send({type:'rpcResult',callId:message.callId,result});
      } else if (message.type==='channelCreate') {
        if (message.endpoint==='loadModel') {
          state.config=message.creationParameter.loadConfigStack.layers[0].config;
          state.loaded=[{...modelInfo,identifier:message.creationParameter.identifier}];
          state.onLoad?.();
          if(state.mode==='config_mismatch') state.config.fields=state.config.fields.map(f=>f.key==='llm.load.contextLength'?{...f,value:f.value-1}:f);
          state.finishLoad=()=>{packet(message.channelId,{type:'success',info:state.loaded[0]});send({type:'channelClose',channelId:message.channelId});};
          if (state.mode==='load_hang') return;
          state.finishLoad();
        } else if (message.endpoint==='predict') {
          prediction=message.channelId;
          state.finishPrediction=()=>settle(prediction,'eosFound');
          state.onPredict?.();
          if (state.mode==='hang' || state.mode==='ignore_cancel') return;
          if (state.mode==='truncate') settle(prediction,'maxPredictedTokensReached'); else settle(prediction,'eosFound');
        } else throw new Error('UNEXPECTED_SYNTHETIC_CHANNEL');
      } else if (message.type==='channelSend' && message.message.type==='cancel') {
        state.cancelled++;
        if (state.mode!=='ignore_cancel' && prediction!==undefined) settle(prediction,'userStopped');
      }
    });
  });
  return { state, url:`ws://127.0.0.1:${server.address().port}`, close:async()=>{
    for (const client of server.clients) client.terminate(); await new Promise(resolve=>server.close(resolve));
  } };
}
// Native seam tests spawn only this harness-owned process; no real LMS/model proof.
if (process.argv[2]==='--stdio-fixture') {
  const own=await endpoint(); process.stdout.write(own.url+'\n');
  own.state.onPredict=()=>process.stdout.write('predict_received\n');
  own.state.onLoad=()=>{if(own.state.emitLoad)process.stdout.write('load_received\n');};
  const input=createInterface({input:process.stdin});
  input.on('line',line=>{
    if(line==='stats'){
      const count=(type,endpoint)=>own.state.messages.filter(m=>m.type===type&&m.endpoint===endpoint).length;
      const last=own.state.messages.findLast(m=>m.type==='channelCreate'&&m.endpoint==='predict');
      const fields=last?.creationParameter.predictionConfigStack?.layers.flatMap(l=>l.config.fields)??[];
      process.stdout.write('stats '+JSON.stringify({loads:count('channelCreate','loadModel'),predictions:count('channelCreate','predict'),uploads:count('rpcCall','uploadFileBase64'),cancelled:own.state.cancelled,loaded:own.state.loaded.length,temperature:fields.find(f=>f.key==='llm.prediction.temperature')?.value,maxTokens:fields.find(f=>f.key==='llm.prediction.maxPredictedTokens')?.value})+'\n');return;
    }
    if(line==='observe_load'){own.state.emitLoad=true;process.stdout.write('mode_set\n');return;}
    if(line==='finish'){own.state.finishPrediction?.();process.stdout.write('finished\n');return;}
    if(line==='finish_after_gap'){setTimeout(()=>own.state.finishPrediction?.(),2000);process.stdout.write('finish_scheduled\n');return;}
    if(line==='finish_load'){own.state.finishLoad?.();process.stdout.write('finished\n');return;}
    if (!['complete','hang','ignore_cancel','invalid_json','truncate','load_hang','config_mismatch'].includes(line)) throw new Error('INVALID_OWN_CONTROL');
    own.state.mode=line;process.stdout.write('mode_set\n');
  });
  input.on('close',async()=>{await own.close();process.exit(0);});
}
