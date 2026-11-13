// Agent evals (AgentEvalEntities.xml): experiments run a dataset through an agent or a service and score each output;
// live scoring scores a sample of an agent's finished runs. One item or run that fails is logged and the rest go on.
const P='moqui.agent.'
const find=(n,c={})=>ec.entity.find(P+n).condition(c).disableAuthz(),one=(n,c)=>find(n,c).one(),rows=(n,c={},order)=>Array.from((order?find(n,c).orderBy(order):find(n,c)).list())
const map=r=>r?.getMap?r.getMap():Object.assign({},r||{}),num=v=>v==null||v===''?null:typeof v=='object'&&v.toNumber?v.toNumber():Number(String(v))
const ms=v=>v==null||v===''?null:Number.isFinite(Number(String(v)))?Number(String(v)):Date.parse(String(v)),now=()=>ec.user.nowTimestamp
const call=(n,p)=>{const out=ec.service.sync().name(n).parameters(p).disableAuthz().call()||{};if(ec.message.hasError())throw new Error(ec.message.getErrorsString());return out}
const create=(n,p)=>call('create#'+P+n,p),update=(n,p)=>call('update#'+P+n,p)
const json=t=>{if(t==null||t==='')return null;if(typeof t!='string')return t;try{return JSON.parse(t)}catch(e){return null}}
const text=v=>v==null?'':typeof v=='string'?v:JSON.stringify(v)
function need(permission){if(!ec.user.userId)throw new Error('Sign in to use evals');if(!ec.user.hasPermission(permission))throw new Error('This needs the '+permission+' permission')}
const TERMINAL={AgrCompleted:1,AgrFailed:1,AgrCanceled:1,AgrRejected:1}

// ---- datasets: each change raises versionNum; an item belongs to versions fromVersion..thruVersion (exclusive)
function dataset(id){const d=one('AgentDataset',{datasetId:id});if(!d)throw new Error('Dataset '+id+' is not available');return d}
function bump(d){const v=(num(d.versionNum)||0)+1;update('AgentDataset',{datasetId:d.datasetId,versionNum:v});return v}
function itemsAt(datasetId,version){return rows('AgentDatasetItem',{datasetId},'createdDate').filter(i=>(num(i.fromVersion)||0)<=version&&(i.thruVersion==null||num(i.thruVersion)>version))}
function addItem(p){need('MONITOR_EVAL');const d=dataset(p.datasetId);if(!String(p.inputText||'').trim())throw new Error('Give the input');const v=bump(d)
 const out=create('AgentDatasetItem',{datasetId:d.datasetId,inputText:p.inputText,groundTruth:p.groundTruth||null,metadataJson:p.metadataJson||null,sourceRunId:p.sourceRunId||null,fromVersion:v,createdDate:now()});return{itemId:out.itemId,versionNum:v}}
function updateItem(p){need('MONITOR_EVAL');const old=one('AgentDatasetItem',{itemId:p.itemId});if(!old||old.thruVersion!=null)throw new Error('Item is not current');const d=dataset(old.datasetId),v=bump(d)
 update('AgentDatasetItem',{itemId:old.itemId,thruVersion:v});const out=create('AgentDatasetItem',{datasetId:d.datasetId,inputText:p.inputText??old.inputText,groundTruth:p.groundTruth??old.groundTruth,metadataJson:p.metadataJson??old.metadataJson,sourceRunId:old.sourceRunId,fromVersion:v,createdDate:now()});return{itemId:out.itemId,versionNum:v}}
function removeItem(p){need('MONITOR_EVAL');const old=one('AgentDatasetItem',{itemId:p.itemId});if(!old||old.thruVersion!=null)throw new Error('Item is not current');const v=bump(dataset(old.datasetId));update('AgentDatasetItem',{itemId:old.itemId,thruVersion:v});return{versionNum:v}}

// ---- runs: the user's message and the reply
// the user message of the run, else the conversation's last user message sent by the time the run was submitted
function runInput(runId){let m=rows('AgentMessage',{runId,role:'user'},'messageSeq')[0]
 if(!m){const run=one('AgentRun',{runId}),at=ms(run?.submittedDate)??Infinity;m=run?.conversationId?rows('AgentMessage',{conversationId:run.conversationId,role:'user'},'-messageSeq').find(r=>(ms(r.createdDate)??0)<=at+1000):null}
 if(!m)return'';return rows('AgentMessagePart',{messageId:m.messageId},'partSeq').filter(r=>r.partKindEnumId==='AgpkText'&&r.textValue).map(r=>r.textValue).join('\n')}
function runOutput(run){const r=json(run.resultJson);return r?.text??r?.output??(r&&typeof r=='object'?JSON.stringify(r):run.resultJson)??(run.errorText||'')}
function saveRun(p){need('MONITOR_EVAL');const run=one('AgentRun',{runId:p.runId});if(!run)throw new Error('Run '+p.runId+' is not available')
 return addItem({datasetId:p.datasetId,inputText:runInput(run.runId),groundTruth:p.groundTruth||(p.useOutput==='Y'?runOutput(run):null),metadataJson:JSON.stringify({agentId:run.agentId}),sourceRunId:run.runId})}

// ---- scoring
const JUDGE='You grade the output of an AI agent. Read the rubric, the input, the expected answer (if any) and the output. Think about it in reasoning, then give score: a number from 0 (fails the rubric) to 1 (fully meets it).'
function judge(s,c){
 const prompt=['## Rubric',s.rubric||'Is the output correct and helpful for the input?','## Input',c.input,c.groundTruth?'## Expected\n'+c.groundTruth:'','## Output',c.output].filter(Boolean).join('\n\n')
 // the eval-judge feature's model when bound (System › LLM), else config.model, else the default model
 const conf=json(s.configJson)||{},bound=ec.entity.find('moqui.llm.LlmFeatureBinding').condition({featureId:'eval-judge'}).disableAuthz().one()
 const r=ec.llm.complete({system:JUDGE,prompt,feature:bound?'eval-judge':undefined,model:bound?undefined:conf.model,schemaName:'score',schema:{type:'object',properties:{reasoning:{type:'string'},score:{type:'number'}},required:['reasoning','score'],additionalProperties:false}})
 const j=r.json||json(r.text)||{};return{score:num(j.score),reason:j.reasoning,judgeJson:JSON.stringify(j),cost:num(r.cost)}
}
function scoreOne(s,c){
 if(s.scorerTypeEnumId==='AgstJudge')return judge(s,c)
 if(s.scorerTypeEnumId==='AgstService'){const out=call(s.serviceName,{input:c.input,output:c.output,groundTruth:c.groundTruth,runId:c.runId,metadata:json(c.metadataJson),config:json(s.configJson)||{}});return{score:num(out.score),reason:out.reason}}
 return null // a human scorer is scored by a person
}
// Writes an AgentScore per scorer; a scorer that fails writes a score row with its error and the others go on.
function score(scorers,c,source){
 const out=[]
 for(const s of scorers){
  let row={scorerId:s.scorerId,sourceEnumId:source,runId:c.runId||null,experimentId:c.experimentId||null,resultId:c.resultId||null,itemId:c.itemId||null,scoredByUserId:ec.user.userId||null,scoredDate:now()}
  try{const r=scoreOne(s,c);if(!r)continue;if(r.score==null||!Number.isFinite(r.score))throw new Error('The scorer gave no score')
   const v=Math.max(0,Math.min(1,r.score)),pass=num(s.passThreshold)??0.5;Object.assign(row,{score:v,passed:v>=pass?'Y':'N',reason:text(r.reason).slice(0,4000)||null,judgeJson:r.judgeJson||null})
  }catch(e){ec.logger.error('Scorer '+s.scorerId+' failed on '+(c.resultId||c.runId)+': '+e);row.errorText=String(e).slice(0,4000)}
  create('AgentScore',row);out.push(row)
 }
 return out
}
const active=a=>(a.thruDate==null||ms(a.thruDate)>ms(now()))&&(a.fromDate==null||ms(a.fromDate)<=ms(now()))
function scorersOf(cond){return rows('AgentScorerAttach',cond).filter(active).map(a=>one('AgentScorer',{scorerId:a.scorerId})).filter(Boolean)}
function scoreRun(p){need('MONITOR_EVAL');const run=one('AgentRun',{runId:p.runId});if(!run)throw new Error('Run '+p.runId+' is not available')
 const ids=p.scorerId?[p.scorerId]:[],scorers=ids.length?ids.map(id=>one('AgentScorer',{scorerId:id})).filter(Boolean):scorersOf({agentId:run.agentId})
 if(!scorers.length)throw new Error('Choose a scorer, or attach one to agent '+run.agentId)
 const rows_=score(scorers,{runId:run.runId,input:runInput(run.runId),output:runOutput(run),groundTruth:p.groundTruth||null},'AgssTrace');return{scoredCount:rows_.length}}
function review(p){need('MONITOR_EVAL');const s=one('AgentScorer',{scorerId:p.scorerId});if(!s)throw new Error('Choose a scorer');const v=num(p.score);if(v==null||v<0||v>1)throw new Error('A score is from 0 to 1')
 let c={runId:p.runId||null};if(p.resultId){const r=one('AgentExperimentResult',{resultId:p.resultId});if(!r)throw new Error('Result is not available');c={runId:r.runId,experimentId:r.experimentId,resultId:r.resultId,itemId:r.itemId}}
 const out=create('AgentScore',Object.assign(c,{scorerId:s.scorerId,sourceEnumId:'AgssReview',score:v,passed:v>=(num(s.passThreshold)??0.5)?'Y':'N',reason:p.reason||null,scoredByUserId:ec.user.userId,scoredDate:now()}))
 if(c.experimentId)summarize(one('AgentExperiment',{experimentId:c.experimentId}));return{scoreId:out.scoreId}}

// ---- experiments
function start(p){
 need('MONITOR_EVAL_RUN');const d=dataset(p.datasetId),version=num(d.versionNum)||0,items=itemsAt(d.datasetId,version),trials=Math.max(1,Math.min(10,Number(p.trials)||1))
 const agentId=p.serviceName?null:(p.agentId||d.agentId);if(!agentId&&!p.serviceName)throw new Error('Choose an agent or a service to run')
 if(agentId&&!one('AgentDefinition',{agentId}))throw new Error('Agent '+agentId+' is not available')
 if(!items.length)throw new Error('The dataset has no items');if(items.length*trials>200)throw new Error('An experiment runs at most 200 items × trials')
 const e=create('AgentExperiment',{experimentName:p.experimentName||d.datasetName+' v'+version,datasetId:d.datasetId,datasetVersion:version,agentId,serviceName:p.serviceName||null,trials,statusId:'EvRunning',startedByUserId:ec.user.userId,startedDate:now(),resultCount:items.length*trials,doneCount:0,failedCount:0,notes:p.notes||null})
 for(const item of items)for(let t=1;t<=trials;t++){
  const r=create('AgentExperimentResult',{experimentId:e.experimentId,itemId:item.itemId,trialNum:t,statusId:'EvrPending'})
  if(!agentId)continue
  // agent runs start now, as this user (ec.llm.startRun needs a user and AgentAdmin); the job reads them as they end
  try{const runId=ec.llm.startRun(agentId,item.inputText);update('AgentExperimentResult',{resultId:r.resultId,runId,statusId:'EvrRunning',startedDate:now()})}
  catch(err){ec.logger.error('Experiment '+e.experimentId+' item '+item.itemId+' did not start: '+err);update('AgentExperimentResult',{resultId:r.resultId,statusId:'EvrFailed',errorText:String(err).slice(0,4000),endedDate:now()})}
 }
 const x=one('AgentExperiment',{experimentId:e.experimentId});if(!agentId)advanceOne(x,5);else summarize(x)
 return{experimentId:e.experimentId}
}
function finishResult(e,r,item,output,cost,scorers){
 update('AgentExperimentResult',{resultId:r.resultId,outputText:output,cost:cost??null,statusId:'EvrScored',endedDate:now()})
 score(scorers,{experimentId:e.experimentId,resultId:r.resultId,itemId:r.itemId,runId:r.runId,input:item?.inputText||'',output,groundTruth:item?.groundTruth,metadataJson:item?.metadataJson},'AgssExperiment')
}
// Reads the runs that ended, calls up to serviceLimit service targets, scores, and ends the experiment when all are done.
function advanceOne(e,serviceLimit){
 const scorers=scorersOf({datasetId:e.datasetId});let calls=0
 for(const r of rows('AgentExperimentResult',{experimentId:e.experimentId})){
  if(r.statusId!=='EvrRunning'&&r.statusId!=='EvrPending')continue
  const item=one('AgentDatasetItem',{itemId:r.itemId})
  try{
   if(r.runId){const run=one('AgentRun',{runId:r.runId});if(!run||!TERMINAL[run.statusId])continue
    if(run.statusId!=='AgrCompleted'){update('AgentExperimentResult',{resultId:r.resultId,statusId:'EvrFailed',errorText:(run.statusId+': '+(run.errorText||run.statusReason||'')).slice(0,4000),cost:num(run.cost),endedDate:now()});continue}
    finishResult(e,r,item,runOutput(run),num(run.cost),scorers)
   }else if(e.serviceName&&r.statusId==='EvrPending'){if(calls++>=serviceLimit)continue
    update('AgentExperimentResult',{resultId:r.resultId,statusId:'EvrRunning',startedDate:now()})
    const out=call(e.serviceName,{input:item?.inputText,metadata:json(item?.metadataJson)})
    finishResult(e,r,item,text(out.output??out.text??out),null,scorers)
   }
  }catch(err){ec.logger.error('Experiment '+e.experimentId+' result '+r.resultId+' failed: '+err);update('AgentExperimentResult',{resultId:r.resultId,statusId:'EvrFailed',errorText:String(err).slice(0,4000),endedDate:now()})}
 }
 summarize(one('AgentExperiment',{experimentId:e.experimentId}))
}
function summarize(e){
 const res=rows('AgentExperimentResult',{experimentId:e.experimentId}),scores=rows('AgentScore',{experimentId:e.experimentId}).filter(s=>s.score!=null)
 const done=res.filter(r=>r.statusId==='EvrScored').length,failed=res.filter(r=>r.statusId==='EvrFailed').length,open=res.length-done-failed
 const mean=scores.length?scores.reduce((a,s)=>a+num(s.score),0)/scores.length:null,pass=scores.length?scores.filter(s=>s.passed==='Y').length/scores.length:null
 const cost=res.reduce((a,r)=>a+(num(r.cost)||0),0),f={experimentId:e.experimentId,doneCount:done,failedCount:failed,meanScore:mean,passRate:pass,cost}
 if(open===0&&e.statusId==='EvRunning')Object.assign(f,{statusId:done?'EvCompleted':'EvFailed',endedDate:now()})
 update('AgentExperiment',f)
}
function advance(p){
 const list=p.experimentId?[one('AgentExperiment',{experimentId:p.experimentId})].filter(Boolean):rows('AgentExperiment',{statusId:'EvRunning'})
 for(const e of list){if(e.statusId!=='EvRunning')continue;try{advanceOne(e,20)}catch(err){ec.logger.error('Experiment '+e.experimentId+' could not advance: '+err)}}
 return{experimentCount:list.length}
}
function cancel(p){need('MONITOR_EVAL_RUN');const e=one('AgentExperiment',{experimentId:p.experimentId});if(!e||e.statusId!=='EvRunning')throw new Error('Experiment is not running')
 for(const r of rows('AgentExperimentResult',{experimentId:e.experimentId}))if(r.statusId==='EvrPending'||r.statusId==='EvrRunning')update('AgentExperimentResult',{resultId:r.resultId,statusId:'EvrFailed',errorText:'Canceled',endedDate:now()})
 update('AgentExperiment',{experimentId:e.experimentId,statusId:'EvCanceled',endedDate:now()});return{}}

// ---- live scoring: a stable sample (a hash of the run id) of each attached agent's completed runs
function sampled(runId,rate){let h=2166136261;for(const ch of String(runId)){h^=ch.charCodeAt(0);h=Math.imul(h,16777619)>>>0}return(h%10000)/10000<rate}
function live(p){
 let scored=0
 for(const a of rows('AgentScorerAttach').filter(a=>a.agentId&&active(a))){
  try{const s=one('AgentScorer',{scorerId:a.scorerId});if(!s||s.scorerTypeEnumId==='AgstHuman')continue
   const rate=num(a.samplingRate)??1,since=a.lastScoredDate||a.fromDate||now()
   const runs=Array.from(find('AgentRun',{agentId:a.agentId,statusId:'AgrCompleted'}).condition('endedDate','greater',since).orderBy('endedDate').limit(100).list())
   for(const run of runs){if(sampled(run.runId,rate)){try{score([s],{runId:run.runId,input:runInput(run.runId),output:runOutput(run)},'AgssLive');scored++}catch(err){ec.logger.error('Live score of run '+run.runId+' failed: '+err)}}}
   if(runs.length)update('AgentScorerAttach',{attachId:a.attachId,lastScoredDate:runs[runs.length-1].endedDate})
  }catch(err){ec.logger.error('Live scoring of attach '+a.attachId+' failed: '+err)}
 }
 return{scoredCount:scored}
}

// ---- built-in service scorers: in input, output, groundTruth, runId, config; out score, reason
const norm=t=>String(t??'').trim().toLowerCase().replace(/\s+/g,' ')
function exactMatch(p){if(!p.groundTruth)return{score:0,reason:'No expected output'};const ok=norm(p.output)===norm(p.groundTruth);return{score:ok?1:0,reason:ok?'Matches the expected output':'Differs from the expected output'}}
// each line of groundTruth (or config.terms) is a phrase the output must contain; the score is the share found
function contains(p){const terms=(p.config?.terms||String(p.groundTruth||'').split('\n')).map(norm).filter(Boolean);if(!terms.length)return{score:0,reason:'No phrases to look for'}
 const out=norm(p.output),missing=terms.filter(t=>!out.includes(t));return{score:(terms.length-missing.length)/terms.length,reason:missing.length?'Missing: '+missing.join('; '):'Contains every phrase'}}
// config.tools (or the lines of groundTruth) are tools the run must have called
function calledTool(p){if(!p.runId)return{score:0,reason:'No run to check'};const want=(p.config?.tools||String(p.groundTruth||'').split('\n')).map(t=>String(t).trim()).filter(Boolean)
 const used=new Set(rows('AgentToolCall',{runId:p.runId}).map(c=>c.toolName));if(!want.length)return{score:used.size?1:0,reason:used.size?'Called '+[...used].join(', '):'Called no tool'}
 const missing=want.filter(t=>!used.has(t));return{score:(want.length-missing.length)/want.length,reason:missing.length?'Did not call '+missing.join(', '):'Called '+want.join(', ')}}

function execute(op,p){return({addItem,updateItem,removeItem,saveRun,scoreRun,review,start,advance,cancel,live,exactMatch,contains,calledTool})[op](p)}
;({execute})
