// SEC-AI-98: a personal skill is its owner's alone. System › LLM › Skills (get#AgentSkill, get#SkillRetrieval) gives an
// AgentAdmin its name and status but not its text, and another user's retrieval not at all; AgentAudit reads both, with
// each read written to AgentAuditLog.
const S='org.moqui.agent.AgentAdminServices.'
const list=(n,p)=>Array.from(ec.entity.find(n).condition(p).disableAuthz().list())
function invoke(n,p){try{const result=call(n,p)||{},error=ec.message.getErrorsString();ec.message.clearErrors();return Object.assign({},result,{error})}catch(e){ec.message.clearErrors();return{error:String(e.message||e)}}}
spec({setupSpec(){login('john.doe');ec.artifactExecution.disableAuthz()
  call('create#moqui.security.UserAccount',{userId:'SKILL_T_OWNER',username:'skill.owner',userFullName:'Skill owner',currentPassword:'16ac58bbfa332c1c55bd98b53e60720bfa90d394',passwordHashType:'SHA'})
  call('create#moqui.security.UserAccount',{userId:'SKILL_T_AUDITOR',username:'skill.auditor',userFullName:'Skill auditor',currentPassword:'16ac58bbfa332c1c55bd98b53e60720bfa90d394',passwordHashType:'SHA'})
  call('create#moqui.security.UserGroup',{userGroupId:'SKILL_T_AUDIT',description:'Skill auditors'})
  for(const userPermissionId of ['AgentAdmin','AgentAudit'])call('create#moqui.security.UserGroupPermission',{userGroupId:'SKILL_T_AUDIT',userPermissionId,fromDate:'2000-01-01T00:00:00Z'})
  call('create#moqui.security.UserGroupMember',{userGroupId:'SKILL_T_AUDIT',userId:'SKILL_T_AUDITOR',fromDate:'2000-01-01T00:00:00Z'})
  // the auditor opens System as an admin does: the screen path is read with authorization on
  call('create#moqui.security.UserGroupMember',{userGroupId:'ADMIN',userId:'SKILL_T_AUDITOR',fromDate:'2000-01-01T00:00:00Z'})
  call('create#moqui.agent.AgentSkill',{skillId:'SKILL_T_SITE',skillName:'skill-t-site',title:'Site steps',skillKind:'guide',
   body:'Nothing private here.',statusId:'AgskPublished',provenance:'admin',versionNum:1})
  call('create#moqui.agent.AgentSkill',{skillId:'SKILL_T_MINE',skillName:'skill-t-private',title:'Private steps',skillKind:'how-to',
   body:'Open the vendor portal with the passphrase in the drawer note.',ownerUserId:'SKILL_T_OWNER',statusId:'AgskPublished',provenance:'learned',versionNum:1})},
 cleanup(){ec.message.clearErrors();ec.artifactExecution.enableAuthz()}})
feature("an AgentAdmin gets another user's personal skill without its text, and AgentAudit reads it with the read logged",{
 when(){login('john.doe');ec.artifactExecution.disableAuthz()
  const admin=call(S+'get#AgentSkill',{skillId:'SKILL_T_MINE'}),retrieval=invoke(S+'get#SkillRetrieval',{query:'vendor portal',userId:'SKILL_T_OWNER'})
  login('skill.auditor');ec.artifactExecution.disableAuthz();let audited,auditedRetrieval
  try{audited=call(S+'get#AgentSkill',{skillId:'SKILL_T_MINE'});auditedRetrieval=invoke(S+'get#SkillRetrieval',{query:'vendor portal',userId:'SKILL_T_OWNER'})}finally{login('john.doe');ec.artifactExecution.disableAuthz()}
  return{admin,retrieval,audited,auditedRetrieval,logs:list('moqui.agent.AgentAuditLog',{auditorUserId:'SKILL_T_AUDITOR'})}},
 then({admin,retrieval,audited,auditedRetrieval,logs}){admin.contentHidden=='Y';!String(admin.skillText??'').includes('passphrase')
  retrieval.error.includes('AgentAudit');audited.skillText.includes('passphrase');auditedRetrieval.error==''
  logs.length==2;logs.every(l=>l.targetUserId=='SKILL_T_OWNER')},
})
// the screens, with authorization on: SkillDetail shows another user's personal skill's text only to an auditor, and
// writes the read; SkillList takes no find on a skill's text, so it tells nothing of what a personal skill says
feature("Skills screens hide a personal skill's text, log the auditor's read, and find nothing by text",{
 when(){login('john.doe');ec.artifactExecution.enableAuthz()
  const render=(path,params)=>{const r=screenTest('apps').render(path,params,'get');return{output:r.output||'',errors:Array.from(r.errorMessages||[])}}
  const admin=render('system/Llm/Skills/SkillDetail',{skillId:'SKILL_T_MINE'})
  const found=render('system/Llm/Skills/SkillList',{skillName:'skill-t-',skillName_op:'begins',body:'passphrase',body_op:'contains'})
  const before=list('moqui.agent.AgentAuditLog',{auditorUserId:'SKILL_T_AUDITOR'}).length
  login('skill.auditor');ec.artifactExecution.enableAuthz();let audited
  try{audited=render('system/Llm/Skills/SkillDetail',{skillId:'SKILL_T_MINE'})}finally{login('john.doe');ec.artifactExecution.disableAuthz()}
  return{admin,found,audited,logged:list('moqui.agent.AgentAuditLog',{auditorUserId:'SKILL_T_AUDITOR'}).length-before}},
 then({admin,found,audited,logged}){admin.errors.length==0;!admin.output.includes('passphrase');admin.output.includes('shown only to its owner')
  found.errors.length==0;found.output.includes('skill-t-site')
  audited.errors.length==0;audited.output.includes('passphrase');logged==1},
})
