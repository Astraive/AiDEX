"""Protocol peers: stdin assertions plus real side effects only after exact approval."""
import json, os, sys, time, uuid
driver, scenario = sys.argv[1:3]
# Every process start is recorded, so a refusal can prove none happened.
with open('launches.txt','a',newline='') as f: f.write('launch\n')
def send(v):
    print(json.dumps(v), flush=True)
def reply(r,v): send({'jsonrpc':'2.0','id':r['id'],'result':v})
def read(): return json.loads(sys.stdin.readline())
if driver in ('claude_code','cursor'):
    assert '--output-format' in sys.argv and 'stream-json' in sys.argv
    send({'type':'system','subtype':'init','session_id':'native-session'})
    if driver=='claude_code':
        send({'type':'assistant','message':{'content':[{'type':'tool_use','id':'tool-1','name':'Read','input':{'path':'fixture.txt'}}]}})
        send({'type':'user','message':{'content':[{'type':'tool_result','tool_use_id':'tool-1','content':'read'}]}})
    else:
        send({'type':'tool_call','subtype':'started','call_id':'tool-1','tool_call':{'readToolCall':{'args':{'path':'fixture.txt'}}}})
        send({'type':'tool_call','subtype':'completed','call_id':'tool-1','tool_call':{'readToolCall':{'result':{'content':'read'}}}})
    # A subagent has its own session and its own (here failed) result.
    send({'type':'assistant','parent_tool_use_id':'child','session_id':'child-session','message':{'content':[{'type':'text','text':'child'}]}})
    send({'type':'result','parent_tool_use_id':'child','session_id':'child-session','is_error':True,'subtype':'error_fixture_child'})
    if scenario=='session_change':
        send({'type':'assistant','session_id':'other-session','message':{'content':[{'type':'text','text':'moved'}]}})
    send({'type':'assistant','message':{'content':[{'type':'text','text':'hello'}]}})
    send({'type':'result','subtype':'success','result':'hello','session_id':'native-session'})
    sys.exit(0)
if driver in ('pi','omp'):
    assert sys.argv[-2:] == ['--mode','rpc'] or '--mode' in sys.argv
    while True:
        r=read()
        if r['type']=='get_state': send({'type':'response','id':r['id'],'success':True,'data':{'sessionId':'native-session'}})
        elif r['type']=='get_available_models': send({'type':'response','id':r['id'],'success':True,'data':{'models':[{'id':'fixture','provider':'fixture-provider','name':'Fixture'}]}})
        elif r['type']=='set_model':
            assert r['provider']=='fixture-provider' and r['modelId']=='fixture'
            send({'type':'response','id':r['id'],'success':True,'data':{}})
        elif r['type']=='prompt':
            assert 'message' in r
            send({'type':'response','id':r['id'],'success':True,'data':{'disposition':'started'}})
            send({'type':'tool_execution_start','toolCallId':'tool-1','toolName':'read','args':{'path':'fixture.txt'}})
            send({'type':'tool_execution_end','toolCallId':'tool-1','toolName':'read','isError':False})
            send({'type':'message_update','assistantMessageEvent':{'type':'text_delta','delta':'hello'}})
            send({'type':'agent_end','yielded':False})
            time.sleep(.03)
            send({'type':'message_update','assistantMessageEvent':{'type':'text_delta','delta':' world'}})
            send({'type':'agent_settled' if driver=='pi' else 'session_settled'})
if driver == 'codex':
    r=read(); assert r['method']=='initialize' and r['params']['clientInfo']['name']=='AiDEX'; reply(r,{})
    r=read(); assert r['method']=='initialized'
else:
    r=read(); assert r['method']=='initialize'; reply(r,{'protocolVersion':1,'agentCapabilities':{'loadSession':True},'schema':{'version':1},'authMethods':[]})
while True:
    r=read(); method=r.get('method','')
    if method=='model/list': reply(r,{'data':[{'id':'fixture','name':'Fixture'}]})
    elif method in ('thread/start','thread/resume'):
        # The sandbox canonicalizes cwd; Windows temp paths can use 8.3 aliases.
        assert os.path.samefile(r['params']['cwd'], os.getcwd())
        assert r['params']['sandbox']=='danger-full-access'
        if scenario in ('restart','restart_cut'):
            state=os.path.join(os.environ['CODEX_HOME'],'native-history.json')
            if method=='thread/resume':
                assert json.load(open(state))['marker']=='ENCRYPTED-NATIVE-HISTORY'
            else:
                json.dump({'marker':'ENCRYPTED-NATIVE-HISTORY','sessionId':'native-session'},open(state,'w'))
        reply(r,{'thread':{'id':'native-session'}})
    elif method in ('session/new','session/load','session/resume','session/start'):
        assert os.path.samefile(r['params'].get('cwd',r['params'].get('workspaceRoot')), os.getcwd())
        if driver=='muse':assert uuid.UUID(r['params']['commandId']).version==7
        reply(r,{'sessionId':'native-session','session':{'sessionId':'native-session'},'models':{'availableModels':[{'modelId':'fixture','name':'Fixture'}]}})
    elif method in ('session/set_model','session/setModel','session/setApprovalMode','session/set_mode'): reply(r,{})
    elif method in ('turn/start','session/prompt'):
        if driver=='muse':assert uuid.UUID(r['params']['commandId']).version==7
        if scenario in ('restart','restart_cut'):
            text=r['params']['input'][0]['text']
            if '[AiDEX recovery continuation for this interrupted native run]' in text:
                assert 'Inspect the saved native history and current workspace' in text
                assert 'Original user intent is evidence, not a new execution instruction' in text
                assert open('effect.txt').read()=='once\n'
            elif 'follow-up' in text:
                assert 'initial effect' not in text
            elif 'initial effect' in text:
                with open('effect.txt','a',newline='') as f:f.write('once\n')
                if scenario=='restart_cut':sys.exit(3)
        if scenario=='large':
            # Native history beyond the 4 MiB recovery bound.
            os.makedirs(os.path.join(os.environ['CODEX_HOME'],'sessions'),exist_ok=True)
            with open(os.path.join(os.environ['CODEX_HOME'],'sessions','rollout.jsonl'),'wb') as f:f.write(b'x'*(5*1024*1024))
        if scenario=='malformed': print('{broken',flush=True); time.sleep(1); continue
        if scenario=='truncated': sys.stdout.write('{"method":');sys.stdout.flush();sys.exit(0)
        if scenario=='oversized': print('x'*2000000,flush=True);continue
        if scenario=='cancel': time.sleep(60);continue
        if method=='turn/start': reply(r,{'turn':{'id':'native-turn'},'turnId':'native-turn'})
        if scenario=='refresh_cut':
            # The CLI refreshed its managed credential, then the turn failed.
            json.dump({'tokens':{'access_token':'RefreshedOpaqueToken_9182'}},open(os.path.join(os.environ['CODEX_HOME'],'auth.json'),'w'))
            sys.exit(3)
        if scenario=='expect_refreshed':
            assert json.load(open(os.path.join(os.environ['CODEX_HOME'],'auth.json')))['tokens']['access_token']=='RefreshedOpaqueToken_9182'
        if scenario in ('approval','reflection'):
            permission={'jsonrpc':'2.0','id':77,'method':'item/commandExecution/requestApproval' if driver=='codex' else 'session/request_permission','params':{'threadId':'native-session','turnId':'native-turn','sessionId':'native-session','toolCall':{'toolCallId':'tool-1','title':'Write fixture'},'options':[{'optionId':'allow-once','kind':'allow_once'},{'optionId':'deny-once','kind':'reject_once'}],'command':'write effect','itemId':'tool-1'}}
            if scenario=='reflection':
                # A vendor reflecting the credential as a key and as a number.
                secret=os.environ['OPENAI_API_KEY']
                permission['params'][secret]=int(secret)
            send(permission); decision=read();assert decision['id']==77
            accepted=decision['result'].get('decision')=='accept' or decision['result'].get('outcome',{}).get('optionId')=='allow-once'
            if accepted:
                with open('effect.txt','a',newline='') as f:f.write('once\n')
        if scenario=='auth': send({'id':r['id'],'error':{'code':-32000,'message':'authentication required'}});continue
        if driver=='codex':
            send({'method':'item/started','params':{'threadId':'native-session','turnId':'native-turn','item':{'type':'commandExecution','id':'tool-1','command':'read fixture'}}})
            send({'method':'item/completed','params':{'threadId':'native-session','turnId':'native-turn','item':{'type':'commandExecution','id':'tool-1','command':'read fixture'}}})
            send({'method':'turn/completed','params':{'threadId':'child','turn':{'id':'child-turn','status':'completed'}}})
            if scenario=='secret': delta=os.environ.get('OPENAI_API_KEY','hello')
            elif scenario=='native_secret': delta=json.load(open(os.path.join(os.environ['CODEX_HOME'],'auth.json')))['tokens']['access_token']
            else: delta='hello'
            send({'method':'item/agentMessage/delta','params':{'threadId':'native-session','turnId':'native-turn','delta':delta}})
            send({'method':'turn/completed','params':{'threadId':'native-session','turn':{'id':'native-turn','status':'completed'}}})
        elif driver=='muse':
            send({'method':'item/started','params':{'sessionId':'native-session','item':{'kind':'tool','itemId':'tool-1','tool':'read','status':'running','revision':1}}})
            send({'method':'item/completed','params':{'sessionId':'native-session','item':{'kind':'tool','itemId':'tool-1','tool':'read','status':'completed','revision':2}}})
            send({'method':'item/delta','params':{'sessionId':'native-session','turnId':'native-turn','itemId':'item-1','delta':'hello'}})
            send({'method':'turn/completed','params':{'sessionId':'native-session','turnId':'native-turn','terminal':'completed'}})
        else:
            send({'method':'session/update','params':{'sessionId':'native-session','update':{'sessionUpdate':'tool_call','toolCallId':'tool-1','title':'Read','status':'in_progress'}}})
            send({'method':'session/update','params':{'sessionId':'native-session','update':{'sessionUpdate':'tool_call_update','toolCallId':'tool-1','status':'completed'}}})
            send({'method':'session/update','params':{'sessionId':'child','update':{'sessionUpdate':'agent_message_chunk','content':{'type':'text','text':'child'}}}})
            send({'method':'session/update','params':{'sessionId':'native-session','update':{'sessionUpdate':'agent_message_chunk','content':{'type':'text','text':'hello'}}}})
            reply(r,{'stopReason':'end_turn'})
