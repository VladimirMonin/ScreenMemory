"""Owned synthetic WebView CDP only; DOM/scripted is NOT genuine input."""
import argparse,ctypes,hashlib,json,pathlib,re,subprocess,time,uuid
from prerequisites import check


def endpoint_outcome(raw, payload):
    if raw == 0 and payload == {'outcome': 'owned'}:
        return 'owned'
    if raw == 3 and payload == {'outcome': 'not_ready', 'fully_qualified_error_id': 'CmdletizationQuery_NotFound,Get-NetTCPConnection'}:
        return 'not_ready'
    raise RuntimeError('Exact endpoint query refused; private raw outputs retained')


def wait_for_endpoint(guard, query, *, clock=time.monotonic, sleep=time.sleep, budget=15):
    if not 0 < budget <= 15:
        raise ValueError('Readiness budget must be bounded by 15 seconds')
    deadline = clock() + budget
    def remaining():
        value = deadline - clock()
        if value <= 0:
            raise TimeoutError('Exact owned endpoint deadline; CAUSE_UNKNOWN')
        return value
    attempts = 0
    while True:
        guard(remaining())
        outcome = query(remaining())
        attempts += 1
        remaining()
        if outcome == 'owned':
            guard(remaining()) # All provenance guards again BEFORE CDP.
            remaining()
            return attempts
        if outcome != 'not_ready':
            raise RuntimeError('Closed readiness outcome refused')
        sleep(min(.5, remaining()))


def main():
    from playwright.sync_api import sync_playwright
    p=argparse.ArgumentParser()
    for name in ['config','output','config-sha256','image-sha256','library-id','canonical-sha256','record-id']:p.add_argument('--'+name,required=True)
    for name in ['pid','port','width','height','created-ticks']:p.add_argument('--'+name,type=int,required=True)
    a=p.parse_args();check()
    repo=pathlib.Path(__file__).resolve().parents[2]
    config=pathlib.Path(a.config).resolve(strict=True);config_bytes=config.read_bytes();assert hashlib.sha256(config_bytes).hexdigest()==a.config_sha256
    settings=json.loads(config_bytes);assert settings['library_id']==a.library_id and settings['environment']=='test'
    root=pathlib.Path(settings['root']).resolve(strict=True);container=root.parent
    assert container.parent==repo/'.pi/temporary' and str(uuid.UUID(container.name,version=4))==container.name
    assert config==root/'config/client.json'
    output=pathlib.Path(a.output).resolve(strict=True);assert output.parent==repo/'.pi/temporary' and str(uuid.UUID(output.name,version=4))==output.name
    expected=repo/'.pi/staging-target-t09b/debug/screenmemory-gui.exe'
    assert pathlib.Path(settings['host_executable']).resolve(strict=True)==repo/'.pi/staging-target-t09b/debug/shotd.exe'
    def private_run(label,command,timeout):
        started=time.monotonic()
        (output/(label+'.argv.json')).write_text(json.dumps(command)+'\n')
        raw=None;outcome='query_failed'
        try:
            with (output/(label+'.stdout')).open('xb') as stdout,(output/(label+'.stderr')).open('xb') as stderr:
                # subprocess.run kills/waits only this exact owned helper on timeout.
                result=subprocess.run(command,stdout=stdout,stderr=stderr,timeout=timeout)
                raw=result.returncode;outcome='completed'
        except subprocess.TimeoutExpired:
            outcome='query_timeout'
            raise TimeoutError('Own helper query hung; REFUSE, no retry') from None
        finally:
            receipt=dict(raw_exit=raw,elapsed_seconds=time.monotonic()-started,outcome=outcome)
            for stream in ['stdout','stderr']:
                receipt[stream+'_sha256']=hashlib.sha256((output/(label+'.'+stream)).read_bytes()).hexdigest()
            (output/(label+'.result.json')).write_text(json.dumps(receipt)+'\n')
        return raw,(output/(label+'.stdout')).read_text(encoding='utf-8-sig')
    command=f"$p=Get-CimInstance Win32_Process -Filter 'ProcessId={a.pid}'; if(!$p){{exit 2}}; @{{ExecutablePath=$p.ExecutablePath;CommandLine=$p.CommandLine;CreatedTicks=$p.CreationDate.ToUniversalTime().Ticks}} | ConvertTo-Json -Compress"
    shell=ctypes.WinDLL('shell32',use_last_error=True);shell.CommandLineToArgvW.argtypes=[ctypes.c_wchar_p,ctypes.POINTER(ctypes.c_int)];shell.CommandLineToArgvW.restype=ctypes.POINTER(ctypes.c_wchar_p)
    kernel=ctypes.WinDLL('kernel32',use_last_error=True);kernel.LocalFree.argtypes=[ctypes.c_void_p];kernel.LocalFree.restype=ctypes.c_void_p
    guard_count=0;query_count=0
    def guard(remaining):
        nonlocal guard_count
        guard_count+=1
        raw,text=private_run(f'identity-{guard_count:02d}',['powershell.exe','-NoProfile','-NonInteractive','-Command',command],remaining)
        assert raw==0, 'GUI liveness/provenance refused'
        identity=json.loads(text)
        assert pathlib.Path(identity['ExecutablePath']).resolve(strict=True)==expected and identity['CreatedTicks']==a.created_ticks
        assert hashlib.sha256(expected.read_bytes()).hexdigest()==a.image_sha256
        argc=ctypes.c_int();argv=shell.CommandLineToArgvW(identity['CommandLine'],ctypes.byref(argc));assert argv
        try:args=[argv[i] for i in range(argc.value)]
        finally:kernel.LocalFree(ctypes.cast(argv,ctypes.c_void_p))
        assert args.count('--config')==1
        ci=args.index('--config');assert ci+1<len(args);wire_path=args[ci+1]
        assert pathlib.Path(wire_path).is_absolute() and not any(part in ('.','..') for part in re.split(r'[\\/]',wire_path))
        assert pathlib.Path(wire_path).resolve(strict=True)==config
        for component in [config,*config.parents,expected,*expected.parents,root,container,output]:assert not getattr(component.lstat(),'st_file_attributes',0)&0x400
        assert config.read_bytes()==config_bytes and 0<a.port<65536
    def query(remaining):
        nonlocal query_count
        query_count+=1
        label=f'endpoint-{query_count:02d}'
        raw,text=private_run(label,['powershell.exe','-NoProfile','-NonInteractive','-File',str(repo/'tests/gui/check_endpoint.ps1'),'-OwnedPid',str(a.pid),'-Port',str(a.port)],remaining)
        outcome=endpoint_outcome(raw,json.loads(text))
        (output/(label+'.outcome.json')).write_text(json.dumps(dict(outcome=outcome,port=a.port,owned_pid=a.pid))+'\n')
        return outcome
    started=time.monotonic()
    try:
        attempts=wait_for_endpoint(guard,query)
    except Exception:
        (output/'readiness.json').write_text(json.dumps(dict(outcome='refused',elapsed_seconds=time.monotonic()-started,attempts=query_count,cause='UNKNOWN'))+'\n')
        raise
    (output/'readiness.json').write_text(json.dumps(dict(outcome='owned',elapsed_seconds=time.monotonic()-started,attempts=attempts))+'\n')
    with sync_playwright() as driver:
        browser=driver.chromium.connect_over_cdp(f'http://127.0.0.1:{a.port}',timeout=15000)
        pages=[page for context in browser.contexts for page in context.pages]
        owned=[page for page in pages if page.locator('h1').first.text_content(timeout=3000)=='ScreenMemory'];assert len(owned)==1
        page=owned[0];panel=page.get_by_role('region',name='Поиск',exact=True)
        panel.locator('.search-results button').nth(1).wait_for(timeout=15000)
        panel.get_by_label('Текст',exact=True).fill('Карточка')
        hit=panel.get_by_role('button',name='Карточка AI Off',exact=False);hit.wait_for(timeout=15000)
        assert panel.locator('.search-results button').count()==1 and hit.get_attribute('data-record-id')==a.record_id
        assert 'Сохранено без модели' in hit.text_content()
        # Observe actual renderer-created Blob; no source/response injection or CSP change.
        page.evaluate('''()=>{window.__s06BlobHashes=[];const original=URL.createObjectURL;URL.createObjectURL=function(blob){blob.arrayBuffer().then(bytes=>crypto.subtle.digest('SHA-256',bytes)).then(hash=>window.__s06BlobHashes.push(Array.from(new Uint8Array(hash),b=>b.toString(16).padStart(2,'0')).join('')));return original.call(this,blob);};}''')
        hit.click();image=panel.locator('.preview img');image.wait_for(timeout=15000)
        observed=image.evaluate('(i)=>({complete:i.complete,width:i.naturalWidth,height:i.naturalHeight,blob:i.src.startsWith("blob:")})')
        assert observed==dict(complete=True,width=a.width,height=a.height,blob=True)
        page.wait_for_function('(sha)=>window.__s06BlobHashes.includes(sha)',arg=a.canonical_sha256,timeout=15000)
        page.screenshot(path=str(output/'dom-search-preview.png'),full_page=True)
        image.scroll_into_view_if_needed();assert config.read_bytes()==config_bytes
        report=dict(owned_pid=a.pid,created_ticks=a.created_ticks,config_sha256=a.config_sha256,image_sha256=a.image_sha256,library_id=a.library_id,record_id=a.record_id,mode='own loopback CDP/scripted DOM',genuine_input='NOT_PROVEN',query='Карточка',visible_results=1,canonical_preview=observed,canonical_blob_sha256=a.canonical_sha256)
        with (output/'dom-receipt.json').open('x',encoding='utf-8') as f:json.dump(report,f,ensure_ascii=False,indent=2)
    print('OWNED_GUI_DOM_SEARCH_PREVIEW=PASS; genuine_input=NOT_PROVEN')

if __name__=='__main__': main()
