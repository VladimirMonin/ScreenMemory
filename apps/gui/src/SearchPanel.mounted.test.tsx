// @vitest-environment happy-dom
import { act, StrictMode } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import SearchPanel from './SearchPanel';
import type { SearchReply } from '../../../packages/protocol/src/generated/application-v1';
const invoke = vi.hoisted(() => vi.fn());
vi.mock('@tauri-apps/api/core', () => ({ invoke }));
vi.mock('./KnowledgePanel', () => ({ default: () => null }));
const id = '00000000-0000-4000-8000-000000000001';
function deferred<T>() { let resolve!: (v:T)=>void; let reject!: (v:unknown)=>void; const promise = new Promise<T>((ok,no)=>{resolve=ok;reject=no;}); return {promise,resolve,reject}; }
function page(searchId:string,title:string):SearchReply { return {kind:'page',value:{search_id:searchId,projection_epoch:'1',candidate_depth:100,window_truncated:false,next_cursor:id,items:[{record_id:id,asset_id:id,content_revision:'1',title,excerpt:'synthetic',category_id:null,category_label:null,exact_match:false,lexical_match:true,exact_terms_truncated:false,position:1}]}}; }
let container:HTMLDivElement; let root:Root;
async function tick() { await act(async()=>{ await vi.advanceTimersByTimeAsync(181); }); }
async function fill(value:string) { await act(async()=>{const input=container.querySelector('input')!;Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype,'value')!.set!.call(input,value);input.dispatchEvent(new Event('input',{bubbles:true}));}); }
async function click(text:string) { await act(async()=>{const button=Array.from(container.querySelectorAll('button')).find(b=>b.textContent?.includes(text));expect(button).toBeTruthy();button!.click();}); }
beforeEach(()=>{
  vi.useFakeTimers();invoke.mockReset();
  Object.assign(globalThis,{IS_REACT_ACT_ENVIRONMENT:true});
  // All source/HTML trusted synthetic; resource/network APIs fail closed.
  vi.stubGlobal('fetch',vi.fn(()=>{throw new Error('network forbidden');}));
  vi.stubGlobal('XMLHttpRequest',class { constructor(){throw new Error('network forbidden');} });
  vi.spyOn(URL,'createObjectURL').mockReturnValue('blob:synthetic');vi.spyOn(URL,'revokeObjectURL').mockImplementation(()=>undefined);
  container=document.createElement('div');document.body.appendChild(container);root=createRoot(container);
});
afterEach(async()=>{await act(async()=>{root.unmount();});container.remove();vi.restoreAllMocks();vi.unstubAllGlobals();vi.useRealTimers();});
describe('mounted SearchPanel bounded latest/cancel/cleanup',()=>{
  it('slow A→B→C coalesces B, drains A, ignores stale error/page/preview and cleans StrictMode',async()=>{
    const a=deferred<SearchReply>(),c=deferred<SearchReply>(),d=deferred<SearchReply>(),oldPage=deferred<SearchReply>(),preview=deferred<unknown>();
    const starts:string[]=[];const ids:Record<string,string>={};const cancels:string[]=[];
    invoke.mockImplementation((command:string,args:{request?:{action:string;text?:string;search_id?:string}})=>{
      if(command==='gui_knowledge')return Promise.resolve({kind:'catalog',value:{items:[]}});
      if(command==='gui_asset_preview')return preview.promise;
      const r=args.request!;
      if(r.action==='cancel'){cancels.push(r.search_id!);return Promise.resolve({kind:'cancelled',value:{search_id:r.search_id}});}
      if(r.action==='page')return oldPage.promise;
      starts.push(r.text!);ids[r.text!]=r.search_id!;
      return ({A:a,C:c,D:d} as Record<string,typeof a>)[r.text!]!.promise;
    });
    await act(async()=>{root.render(<StrictMode><SearchPanel ready /></StrictMode>);});
    await fill('A');await tick();expect(starts).toEqual(['A']);
    await fill('B');await tick();await fill('C');await tick();expect(starts).toEqual(['A']);expect(cancels).toEqual([ids.A]);
    await act(async()=>{a.reject('STALE_A_ERROR');});expect(starts).toEqual(['A','C']);expect(container.textContent).not.toContain('STALE_A_ERROR');
    await act(async()=>{c.resolve(page(ids.C,'Latest C'));});expect(container.textContent).toContain('Latest C');
    await click('Latest C');await click('Следующая страница');await fill('D');await tick();
    expect(container.querySelector('img')).toBeNull();expect(container.textContent).not.toContain('Latest C');
    await act(async()=>{preview.reject('STALE_PREVIEW_ERROR');oldPage.resolve(page(ids.C,'STALE_PAGE'));});
    expect(starts).toEqual(['A','C','D']);expect(container.textContent).not.toContain('STALE_PAGE');expect(container.textContent).not.toContain('PREVIEW_UNAVAILABLE');
    await act(async()=>{d.resolve(page(ids.D,'Latest D'));});expect(container.textContent).toContain('Latest D');
    expect(invoke.mock.calls.filter(c=>c[0].includes('processing'))).toHaveLength(0);
    await act(async()=>{root.unmount();});expect(cancels.filter(v=>v===ids.A)).toHaveLength(1);expect(cancels.length).toBeLessThanOrEqual(4);
  });
  it('expired old window cleanup does not lose latest start; not-ready removes its page',async()=>{
    let last='';const starts:string[]=[];
    invoke.mockImplementation((command:string,args:{request?:{action:string;text:string;search_id:string}})=>{
      if(command==='gui_knowledge')return Promise.resolve({kind:'catalog',value:{items:[]}});
      const r=args.request!;if(r.action==='cancel')return Promise.reject('SEARCH_EXPIRED');
      starts.push(r.text);last=r.search_id;return Promise.resolve(page(last,'Title '+r.text));
    });
    await act(async()=>{root.render(<StrictMode><SearchPanel ready /></StrictMode>);});await fill('A');await tick();
    await fill('B');await tick();expect(starts).toEqual(['A','B']);expect(container.textContent).toContain('Title B');expect(container.textContent).not.toContain('SEARCH_EXPIRED');
    await act(async()=>{root.render(<StrictMode><SearchPanel ready={false} /></StrictMode>);});expect(container.querySelector('.search-results button')).toBeNull();
  });
  it('unmount cancels only its active request once and discards late success',async()=>{
    const request=deferred<SearchReply>();let searchId='';let cancelled=0;
    invoke.mockImplementation((command:string,args:{request?:{action:string;search_id:string}})=>{
      if(command==='gui_knowledge')return Promise.resolve({kind:'catalog',value:{items:[]}});
      if(args.request!.action==='cancel'){cancelled++;return Promise.resolve({kind:'cancelled',value:{search_id:args.request!.search_id}});}
      searchId=args.request!.search_id;return request.promise;
    });
    await act(async()=>{root.render(<StrictMode><SearchPanel ready /></StrictMode>);});await fill('A');await tick();
    await act(async()=>{root.unmount();});expect(cancelled).toBe(1);
    await act(async()=>{request.resolve(page(searchId,'TOO_LATE'));});expect(container.textContent).toBe('');expect(cancelled).toBeLessThanOrEqual(2);
  });
});
