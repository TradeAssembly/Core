"""Private synthetic-only compiled Core process gate, never a demo or broker test."""
import os,sys,json,pathlib,subprocess,sqlite3,time,selectors,hashlib
cfg=json.load(sys.stdin);root=pathlib.Path(cfg['root']).resolve();db=root/'service.db';out=pathlib.Path(cfg['evidence']).resolve();out.mkdir(parents=True,exist_ok=False)
assert (root/'SYNTHETIC_SESSION_PROCESS_FIXTURE').read_text()=='synthetic only\n'
binary=pathlib.Path(cfg['binary']).resolve();assert binary.is_file();assert cfg['wardenOrigin'].startswith('http://127.0.0.1:');assert cfg['revision']=='9af48a8cc65d4fc337edcd0e505ab2739039bc77'
env={k:os.environ[k] for k in ['HOME','PATH','TMPDIR'] if k in os.environ};env.update({'TRADEASSEMBLY_AUTH_PROFILE':'local_owner','TRADEASSEMBLY_ARTIFACT_ROOT':str(root/'process-artifacts'),'TRADEASSEMBLY_WARDEN_SIDECAR_URL':cfg['wardenOrigin'],'TRADEASSEMBLY_PLUGIN_SANDBOX_COMMAND':cfg['srt'],'TRADEASSEMBLY_PLUGIN_SANDBOX_ALLOW_LOCAL_EGRESS':'true','TRADEASSEMBLY_TELEMETRY_ENABLED':'false'})
children=[];logs=[]
def save(name,v):
 p=out/(name+'.json');assert not p.exists();p.write_text(json.dumps(v,indent=2)+'\n')
def rows(ns):
 con=sqlite3.connect('file:'+str(db)+'?mode=ro',uri=True,timeout=5)
 try:return [(k,json.loads(v)) for k,v in con.execute('select item_key,value_json from tradeassembly_kv where namespace=? order by item_key',(ns,))]
 finally:con.close()
def until(fn,seconds=120):
 end=time.monotonic()+seconds
 while True:
  result=fn()
  if result:return result
  for p in children:
   if p.poll() is not None:raise AssertionError('owned Core process exited unexpectedly: '+str(p.returncode))
  if time.monotonic()>end:raise AssertionError('bounded process gate timed out')
  time.sleep(.2)
def spawn(kind,name,discard=False):
 stderr=open(out/(name+'.stderr.log'),'wb');logs.append(stderr)
 args=[str(binary),'--db',str(db)]+(['mcp','serve','--transport','stdio'] if kind=='mcp' else ['api','--host','127.0.0.1','--port','0'])
 stdout=subprocess.DEVNULL if discard else (subprocess.PIPE if kind=='mcp' else open(out/(name+'.stdout.log'),'wb'))
 if kind!='mcp':logs.append(stdout)
 p=subprocess.Popen(args,cwd=root,env=env,stdin=subprocess.PIPE if kind=='mcp' else subprocess.DEVNULL,stdout=stdout,stderr=stderr,text=False);children.append(p);return p
def kill(p):
 if p.poll() is None:p.kill()
 p.wait(timeout=20);children.remove(p)
def send(p,msg):p.stdin.write((json.dumps(msg)+'\n').encode());p.stdin.flush()
def receive(p):
 sel=selectors.DefaultSelector();sel.register(p.stdout,selectors.EVENT_READ)
 if not sel.select(120):raise AssertionError('MCP response timeout')
 line=p.stdout.readline();sel.close();assert line,'MCP process closed its output';return json.loads(line)
def call(p,i,args):
 send(p,{'jsonrpc':'2.0','id':i,'method':'tools/call','params':{'name':'tradeassembly.execution.scheduler.run','arguments':args}});v=receive(p);assert v.get('id')==i,v;assert not v.get('error'),v;assert v['result']['isError'] is False,v;return v
original={'sessionDispatch':{'kind':'enqueue','job':cfg['job']},'idempotencyKey':'process-enqueue-original'}
initial_orders=rows('orders');initial_positions=rows('positions');initial_operations=rows('plugin_operation_receipts')
try:
 # Initialize normally, then close the reply pipe before receiving any byte of
 # the enqueue response. The server commits before its stdout write fails.
 lost=spawn('mcp','lost-enqueue');send(lost,{'jsonrpc':'2.0','id':0,'method':'initialize','params':{}});assert receive(lost)['id']==0;send(lost,{'jsonrpc':'2.0','method':'notifications/initialized'});send(lost,{'jsonrpc':'2.0','id':1,'method':'tools/call','params':{'name':'tradeassembly.execution.scheduler.run','arguments':original}});lost.stdout.close()
 stored=until(lambda:next((v for _,v in rows('session_dispatch_jobs_v1') if v['request']==cfg['job'] and v.get('queueMessageId')),None));assert stored['coreRevision']==cfg['revision'];job_id=stored['jobId']
 until(lambda:any(v.get('envelope',{}).get('idempotency_key')=='process-enqueue-original' and v.get('response_status')==200 for _,v in rows('control_plane_commands')))
 kill(lost);save('lost-response-original-job',stored)
 mcp=spawn('mcp','replay-and-enable');send(mcp,{'jsonrpc':'2.0','id':0,'method':'initialize','params':{}});assert receive(mcp)['id']==0;send(mcp,{'jsonrpc':'2.0','method':'notifications/initialized'});replay=call(mcp,2,original);assert replay['result']['structuredContent']['body']['job']['jobId']==job_id,replay;assert replay['result']['structuredContent']['body']['job']['createdAtMs']==stored['createdAtMs'];assert len(rows('session_dispatch_jobs_v1'))==1;assert rows('plugin_operation_receipts')==initial_operations
 enabled=call(mcp,3,{'sessionDispatch':{'kind':'start_worker','jobId':job_id},'idempotencyKey':'process-start-worker-original'});save('mcp-replay',replay);save('mcp-enable',enabled)
 intent=rows('session_dispatch_worker_intents_v1');assert len(intent)==1 and intent[0][1]['enabled']
 before=rows('session_dispatch_jobs_v1')[0][1];assert before['startedAtMs'] is None,'fixture setup consumed scheduled window before restart';assert rows('plugin_operation_receipts')==initial_operations
 first_pid=mcp.pid;kill(mcp);save('intent-before-core-restart',{'intent':intent,'job':before,'stoppedPid':first_pid})
 api=spawn('api','normal-api-startup');api_pid=api.pid
 final=until(lambda:next((v for _,v in rows('session_dispatch_jobs_v1') if v['jobId']==job_id and v['status'] in ('complete','failed','partial','uncertain','skipped_missed')),None),300)
 assert final['status']=='complete',final;assert final['createdAtMs']==stored['createdAtMs'];assert final['coreRevision']==cfg['revision'];assert rows('session_dispatch_worker_intents_v1')==intent
 evaluations=rows('data_evaluation_receipts_v1');assert len(evaluations)==1;receipt=evaluations[0][1];assert receipt['status']=='ready' and receipt['framesProcessed']==2;assert receipt['signals'][0]['entry']['status']=='warming_up';assert receipt['signals'][1]['entry']=={'status':'ready','value':True};assert all(s['tradeInstruments'][0]['instrumentId']=='A' for s in receipt['signals']);assert rows('orders')==initial_orders and rows('positions')==initial_positions
 save('normal-api-startup-completed-job',final);save('normal-api-startup-evaluation',receipt);kill(api)
 operations=rows('plugin_operation_receipts');finished_job=rows('session_dispatch_jobs_v1');api2=spawn('api','completed-job-restart');time.sleep(3);assert api2.poll() is None;assert rows('plugin_operation_receipts')==operations;assert rows('session_dispatch_jobs_v1')==finished_job;assert rows('orders')==initial_orders;kill(api2)
 save('result',{'status':'PASS','coreSourceRevision':cfg['revision'],'binarySha256':hashlib.sha256(binary.read_bytes()).hexdigest(),'wholeResponseLoss':'MCP reply pipe closed after initialization before enqueue response; committed original request replayed in fresh process','actualCoreProcessRestart':'MCP worker process terminated before due; normal CLI API startup resumed unchanged enabled intent and evaluated native plugins through SRT','apiPid':api_pid,'completedRestartNoNewProviderInvocation':True,'originalJobAndIntentPreserved':True,'nativeFrames':2,'warmingThenReady':True,'provider':'authored loopback fixture','identity':'protected synthetic installation owner','warden':'real HTTP adapter to synthetic mock','srtLocalEgress':'explicit loopback fixture opt-in','brokerOrders':False,'releaseQualified':False,'relayTransportResponseLoss':'NOT exercised by this Core MCP case'})
 print(json.dumps({'status':'PASS','jobId':job_id,'evidence':str(out)}))
finally:
 for p in children[:]:
  try:kill(p)
  except Exception:pass
 for f in logs:f.close()
