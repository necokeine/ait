# Offline app-server protocol fixture. Configuration is prepended by each Rust test.
import json
import os
import sys
import time

turns = []
pending = None
thread_id = 'thread-a' if APPROVAL else 'thread-http-test'
turn_id = 'turn-a' if APPROVAL else 'turn-http-test'

def send(value):
    print(json.dumps(value, separators=(',', ':')), flush=True)

def thread():
    return dict(id=thread_id, sessionId=thread_id, cwd=os.getcwd(), source='appServer',
                preview='', historyMode='paginated', status=dict(type='idle'), createdAt=1, updatedAt=2, turns=[])

def item(value, method='item/completed'):
    send(dict(method=method, params=dict(threadId=thread_id, turnId=turn_id, item=value)))

def finish():
    items = [dict(id='user', type='userMessage', clientId=pending.get('clientUserMessageId'), content=pending['input'])]
    if not APPROVAL:
        item(dict(type='agentMessage', id='commentary-http-test', phase='commentary', text='Inspecting the project.'))
        item(dict(type='commandExecution', id='command-http-test', status='inProgress', command='pwd'), 'item/started')
        item(dict(type='agentMessage', id='stress-http-test', phase='commentary', text=''), 'item/started')
        for _ in range(4000):
            send(dict(method='item/agentMessage/delta', params=dict(threadId=thread_id, turnId=turn_id, itemId='stress-http-test', delta='x')))
        time.sleep(.6)
        operation = dict(type='commandExecution', id='command-http-test', status='completed', command='pwd', aggregatedOutput='project')
        item(operation)
        items.append(operation)
        time.sleep(.6)
    answer = dict(type='agentMessage', id='answer', phase='final_answer', text=ANSWER)
    item(answer)
    items.append(answer)
    turns.append(dict(id=turn_id, status='completed', error=None, itemsView='full', startedAt=1, completedAt=2, items=items))
    send(dict(method='turn/completed', params=dict(threadId=thread_id, turn=dict(id=turn_id, status='completed', items=[]))))

for line in sys.stdin:
    request = json.loads(line)
    if LOG:
        with open(LOG, 'a') as log:
            log.write(json.dumps(request, separators=(',', ':'))+'\n')
    method, identity = request.get('method'), request.get('id')
    params = request.get('params', {})
    if method == 'initialize':
        send(dict(id=identity, result={}))
    elif method == 'model/list':
        send(dict(id=identity, result=dict(data=[dict(model='gpt-5.6-sol', displayName='Codex Test', supportedReasoningEfforts=[dict(reasoningEffort='high')])], nextCursor=None)))
    elif method in ('thread/start', 'thread/resume'):
        sandbox = {'read-only':dict(type='readOnly',networkAccess=False), 'workspace-write':dict(type='workspaceWrite',writableRoots=[os.getcwd()],networkAccess=False,excludeTmpdirEnvVar=False,excludeSlashTmp=False), 'danger-full-access':dict(type='dangerFullAccess')}[params['sandbox']]
        send(dict(id=identity, result=dict(thread=thread(),model=params['model'],modelProvider='openai',reasoningEffort='high',cwd=os.getcwd(),sandbox=sandbox,approvalPolicy=params['approvalPolicy'],approvalsReviewer='user')))
    elif method == 'thread/read':
        assert params['includeTurns'] is False
        send(dict(id=identity,result=dict(thread=thread())))
    elif method == 'thread/turns/list':
        send(dict(id=identity,result=dict(data=turns,nextCursor=None)))
    elif method == 'turn/start':
        pending = params
        send(dict(id=identity,result=dict(turn=dict(id=turn_id))))
        while globals().get("GATE") and not os.path.exists(GATE):
            time.sleep(.01)
        time.sleep(DELAY)
        if APPROVAL:
            send(dict(id=73, method='item/commandExecution/requestApproval', params=dict(threadId=thread_id,turnId=turn_id,itemId='command-a',command='curl -H X-Api-Key:header-secret --header="Authorization: Bearer auth-secret" -H "Cookie: session=cookie-secret" https://url-user:url-secret@example.test/v1',cwd=os.getcwd(),reason='offline fixture')))
        else:
            finish()
    elif identity == 73 and 'result' in request:
        assert request['result']['decision'] == 'accept'
        finish()
