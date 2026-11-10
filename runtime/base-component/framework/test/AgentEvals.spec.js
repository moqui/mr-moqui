// Agent evals (AgentEval.js): dataset versions, a service-target experiment scored by the built-in scorers, scoring a run
// and live sampling of an agent's runs, a person's review, and the permission to start experiments. No model calls.
const S='org.moqui.agent.AgentEvalServices.'
const one=(n,p)=>ec.entity.find(n).condition(p).disableAuthz().one(),list=(n,p)=>Array.from(ec.entity.find(n).condition(p).disableAuthz().list())
const num=v=>v==null?null:v.toNumber?v.toNumber():Number(v)
function invoke(n,p){try{const result=call(n,p)||{},error=ec.message.getErrorsString();ec.message.clearErrors();return Object.assign({},result,{error})}catch(e){ec.message.clearErrors();return{error:String(e.message||e)}}}
spec({setupSpec(){login('john.doe');ec.artifactExecution.disableAuthz()
  call('create#moqui.agent.AgentDefinition',{agentId:'EVAL_T_AGENT',agentName:'Eval test agent',surfaces:'job'})
  call('create#moqui.agent.AgentDataset',{datasetId:'EVAL_T_DS',datasetName:'Eval test',versionNum:0,createdDate:ec.user.nowTimestamp})
  call('create#moqui.security.UserAccount',{userId:'EVAL_T_VIEWER',username:'eval.viewer',userFullName:'Eval viewer',currentPassword:'16ac58bbfa332c1c55bd98b53e60720bfa90d394',passwordHashType:'SHA'})},
 cleanup(){ec.message.clearErrors();ec.artifactExecution.enableAuthz()}})
let firstItem
feature('items change by version, and an experiment runs the version current when it starts',{
 when(){login('john.doe');ec.artifactExecution.disableAuthz();const a=call(S+'add#DatasetItem',{datasetId:'EVAL_T_DS',inputText:'first',groundTruth:'No expected'}),b=call(S+'add#DatasetItem',{datasetId:'EVAL_T_DS',inputText:'second',groundTruth:'No expected output\nnot there'})
  const c=call(S+'add#DatasetItem',{datasetId:'EVAL_T_DS',inputText:'third'}),removed=call(S+'remove#DatasetItem',{itemId:c.itemId}),edited=call(S+'update#DatasetItem',{itemId:a.itemId,groundTruth:'No expected output'})
  firstItem=edited.itemId;return{a,edited,removed,old:one('moqui.agent.AgentDatasetItem',{itemId:a.itemId}),ds:one('moqui.agent.AgentDataset',{datasetId:'EVAL_T_DS'})}},
 then({a,edited,removed,old,ds}){removed.versionNum==4;edited.versionNum==5;num(old.thruVersion)==5;edited.itemId!=a.itemId;num(ds.versionNum)==5},
})
feature('a service-target experiment scores each output with the dataset scorers and ends with its mean and pass rate',{
 when(){login('john.doe');ec.artifactExecution.disableAuthz();for(const scorerId of ['CONTAINS','EXACT_MATCH'])call('create#moqui.agent.AgentScorerAttach',{scorerId,datasetId:'EVAL_T_DS',fromDate:'2000-01-01T00:00:00Z'})
  // score#ExactMatch with no groundTruth answers {score:0, reason:'No expected output'}: a fixed output to score
  const started=call(S+'start#Experiment',{datasetId:'EVAL_T_DS',serviceName:S+'score#ExactMatch',trials:1}),e=one('moqui.agent.AgentExperiment',{experimentId:started.experimentId})
  return{e,results:list('moqui.agent.AgentExperimentResult',{experimentId:e.experimentId}),scores:list('moqui.agent.AgentScore',{experimentId:e.experimentId})}},
 then({e,results,scores}){e.statusId=='EvCompleted';num(e.datasetVersion)==5;results.length==2;results.every(r=>r.statusId==='EvrScored'&&r.outputText.includes('No expected output'))
  scores.length==4;scores.every(s=>s.sourceEnumId==='AgssExperiment'&&s.errorText==null);scores.filter(s=>s.scorerId==='CONTAINS').map(s=>num(s.score)).sort().join()=='0.5,1';num(e.passRate)==0.25;Math.abs(num(e.meanScore)-0.375)<1e-9},
})
let runId
feature('a run is scored on its reply and tool calls, and live scoring samples the attached agent once',{
 when(){login('john.doe');ec.artifactExecution.disableAuthz();runId=call('create#moqui.agent.AgentRun',{agentId:'EVAL_T_AGENT',userId:ec.user.userId,statusId:'AgrCompleted',submittedDate:'2026-01-01T00:00:00Z',endedDate:new Date(Date.now()-1000).toISOString(),resultJson:JSON.stringify({text:'The order ships Tuesday.'})}).runId
  const m=call('create#moqui.agent.AgentMessage',{conversationId:'EVAL_T_CONV',messageSeq:1,runId,role:'user',createdDate:ec.user.nowTimestamp})
  call('create#moqui.agent.AgentMessagePart',{messageId:m.messageId,partSeq:1,partKindEnumId:'AgpkText',textValue:'When does it ship?'})
  call('create#moqui.agent.AgentToolCall',{runId,stepSeq:1,toolName:'find_order'})
  const scored=call(S+'score#Run',{runId,scorerId:'CALLED_TOOL',groundTruth:'find_order\nship_order'})
  call('create#moqui.agent.AgentScorerAttach',{scorerId:'CONTAINS',agentId:'EVAL_T_AGENT',samplingRate:1,fromDate:'2000-01-01T00:00:00Z',lastScoredDate:'2026-01-01T00:00:00Z'})
  const live1=call(S+'score#LiveRuns',{}),live2=call(S+'score#LiveRuns',{}),saved=call(S+'save#RunAsDatasetItem',{runId,datasetId:'EVAL_T_DS',useOutput:'Y'})
  return{scored,live1,live2,scores:list('moqui.agent.AgentScore',{runId}),item:one('moqui.agent.AgentDatasetItem',{itemId:saved.itemId})}},
 then({scored,live1,live2,scores,item}){scored.scoredCount==1;live1.scoredCount==1;live2.scoredCount==0
  const tool=scores.find(s=>s.sourceEnumId==='AgssTrace');num(tool.score)==0.5;tool.reason.includes('ship_order');scores.some(s=>s.sourceEnumId==='AgssLive')
  item.inputText=='When does it ship?';item.groundTruth=='The order ships Tuesday.';item.sourceRunId==runId},
})
feature('a person reviews a result, and starting an experiment needs MONITOR_EVAL_RUN',{
 when(){login('john.doe');ec.artifactExecution.disableAuthz();const e=list('moqui.agent.AgentExperiment',{datasetId:'EVAL_T_DS'})[0],r=list('moqui.agent.AgentExperimentResult',{experimentId:e.experimentId})[0]
  const bad=invoke(S+'record#ReviewScore',{scorerId:'HUMAN_REVIEW',resultId:r.resultId,score:2}),ok=call(S+'record#ReviewScore',{scorerId:'HUMAN_REVIEW',resultId:r.resultId,score:1,reason:'Fine'})
  login('eval.viewer');ec.artifactExecution.disableAuthz();let denied;try{denied=invoke(S+'start#Experiment',{datasetId:'EVAL_T_DS',serviceName:S+'score#ExactMatch'})}finally{login('john.doe');ec.artifactExecution.disableAuthz()}
  return{bad,score:one('moqui.agent.AgentScore',{scoreId:ok.scoreId}),denied,after:one('moqui.agent.AgentExperiment',{experimentId:e.experimentId})}},
 then({bad,score,denied,after}){bad.error.includes('0 to 1');score.sourceEnumId=='AgssReview';score.passed=='Y';denied.error.includes('MONITOR_EVAL_RUN');num(after.meanScore)==0.5},
})
