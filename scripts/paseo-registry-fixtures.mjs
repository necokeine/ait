// Regenerate with: node scripts/paseo-registry-fixtures.mjs /path/to/paseo /path/to/node_modules/zod
// Runs the pinned upstream Zod definitions; Rust tests do not require Node or Paseo.
import fs from 'node:fs';
import path from 'node:path';
import { createRequire } from 'node:module';
import { execFileSync } from 'node:child_process';
import crypto from 'node:crypto';
const require = createRequire(import.meta.url);
const sourceRoot = process.argv[2];
const zodPath = process.argv[3];
if (!sourceRoot || !zodPath) throw new Error('Pass Paseo checkout and installed zod directory');
const { z } = require(path.resolve(zodPath));
const revision = execFileSync('git', ['-C', sourceRoot, 'rev-parse', 'HEAD'], { encoding: 'utf8' }).trim();
if (revision !== '2c8e8a826810337492cc5a38bb0bbd705b6fb632') throw new Error('Unexpected Paseo revision');
const recordPath = 'packages/server/src/server/workspace-registry.ts';
const messagePath = 'packages/protocol/src/messages.ts';
const records = fs.readFileSync(path.join(sourceRoot, recordPath), 'utf8');
const messages = fs.readFileSync(path.join(sourceRoot, messagePath), 'utf8');
const block = (text, start, end) => text.slice(text.indexOf(start), text.indexOf(end));
const definitions = [
  block(records, 'const UntrustedWorkspaceSourceSchema', 'export type PersistedProjectRecord'),
  block(messages, 'const WorkspaceStateBucketSchema', 'export const FetchWorkspacesRequestMessageSchema'),
  block(messages, 'export const ProjectCheckoutLiteNotGitPayloadSchema', 'export const AgentUpdateMessageSchema'),
  block(messages, 'export const WorkspaceProjectDescriptorPayloadSchema', 'export const FetchWorkspacesResponseMessageSchema'),
].join('\n').replaceAll('export const ', 'const ');
const names = ['PersistedProjectRecord','PersistedWorkspaceRecord','ProjectCheckoutLitePayload','ProjectPlacementPayload','WorkspaceScriptPayload','WorkspaceDescriptorPayload','WorkspaceProjectDescriptorPayload','WorkspaceGitHubRuntimePayload'];
const schemas = Function('z', `${definitions}\nreturn {${names.map(name=>`${name}:${name}Schema`).join(',')}};`)(z);
const time = '2026-03-01T00:00:00.000Z';
const project = {projectId:'remote:github.com/acme/repo',rootPath:'/repo',kind:'git',displayName:'repo',createdAt:time,updatedAt:time,archivedAt:null};
const workspace = {workspaceId:'wks_1234567890abcdef',projectId:project.projectId,cwd:'/repo/sub',kind:'local_checkout',displayName:'main',createdAt:time,updatedAt:time,archivedAt:null};
const checkout = {cwd:'/repo/sub',isGit:true,currentBranch:'main',remoteUrl:null,isPaseoOwnedWorktree:false};
const script = {scriptName:'dev',hostname:'localhost',port:null,lifecycle:'running',health:null};
const descriptor = {id:workspace.workspaceId,projectId:project.projectId,projectDisplayName:'repo',projectRootPath:'/repo',projectKind:'git',workspaceKind:'local_checkout',name:'main',status:'done',activityAt:null};
const pr = {number:42,url:'https://example.invalid/pr/42',title:'Title',state:'OPEN',baseRefName:'main',headRefName:'branch',isMerged:false,isDraft:false,mergeable:'MERGEABLE',checks:[{name:'tests',status:'success',url:null,workflow:'CI',duration:'1m',traits:['required']}],checksStatus:'success',reviewDecision:null,repoOwner:'acme',repoName:'repo',github:{extension:true}};
const full = {
 PersistedProjectRecord:{...project,projectKey:'key',customName:'Custom',customIconRevision:'icon'},
 PersistedWorkspaceRecord:{...workspace,title:'Title',branch:'main',worktreeRoot:'/repo',baseBranch:'refs/remotes/upstream/main',isPaseoOwnedWorktree:true,mainRepoRoot:'/main',autoArchivedChangeRequestUrl:'https://example.invalid/pr/1',pinnedAt:time,labels:['one'],untrustedSource:{kind:'change_request',forge:'github',number:1,headRepository:'acme/repo'}},
 ProjectCheckoutLitePayload:{...checkout,worktreeRoot:'/repo',mainRepoRoot:'/main'},
 ProjectPlacementPayload:{projectKey:'key',projectName:'repo',workspaceName:null,checkout},
 WorkspaceScriptPayload:{...script,type:'script',port:3000,localProxyUrl:null,publicProxyUrl:'https://example.invalid',proxyUrl:null,exitCode:0,terminalId:null},
 WorkspaceProjectDescriptorPayload:{projectId:'prj_1234567890abcdef',projectKey:'key',projectDisplayName:'repo',projectCustomName:null,projectCustomIconRevision:null,projectIconRevision:'icon',projectRootPath:'/repo',projectKind:'non_git',syncSeq:1},
 WorkspaceGitHubRuntimePayload:{featuresEnabled:true,pullRequest:pr,error:{message:'retry'},refreshedAt:null},
 WorkspaceDescriptorPayload:{...descriptor,projectCustomName:null,projectCustomIconRevision:'icon',workspaceDirectory:'/repo/sub',worktreeSlug:'branch',title:null,pinnedAt:time,labels:[],archivingAt:null,statusEnteredAt:null,diffStat:{additions:1,deletions:2},scripts:[script],gitRuntime:{currentBranch:null,remoteUrl:null,isPaseoOwnedWorktree:false,isDirty:true,aheadBehind:{ahead:1,behind:0},aheadOfOrigin:null,behindOfOrigin:0},githubRuntime:{pullRequest:pr},forge:'github',project:{projectKey:'key',projectName:'repo',checkout},syncSeq:1},
};
const cases=[];
function add(schema,name,input) {
 const result=schemas[schema].safeParse(input);
 cases.push({schema,name,input,valid:result.success,...(result.success?{output:JSON.parse(JSON.stringify(result.data))}:{})});
}
for (const [schema,input] of Object.entries(full)) {
 add(schema,'full',input);
 add(schema,'unknown_fields_are_stripped',{...input,unknown:'discard'});
 for (const key of Object.keys(input)) {
  const missing={...input};delete missing[key];add(schema,`missing_${key}`,missing);
  add(schema,`null_${key}`,{...input,[key]:null});
  add(schema,`wrong_type_${key}`,{...input,[key]:{invalid:true}});
 }
}
add('PersistedProjectRecord','minimal',project);
add('PersistedProjectRecord','empty_strings_and_opaque_timestamps',{...project,projectId:'',createdAt:'not-a-date',customName:''});
add('PersistedWorkspaceRecord','minimal',workspace);
for (const number of [0,-1,1.5,Number.MAX_SAFE_INTEGER,Number.MAX_SAFE_INTEGER+1]) {
 add('PersistedWorkspaceRecord',`source_number_${number}`,{...workspace,untrustedSource:{kind:'change_request',forge:'x',number,headRepository:'x'}});
 add('WorkspaceDescriptorPayload',`sync_seq_${number}`,{...descriptor,syncSeq:number});
 add('WorkspaceScriptPayload',`port_${number}`,{...script,port:number});
}
for (const kind of ['git','non_git','directory','checkout','worktree','invalid']) {
 add('PersistedProjectRecord',`kind_${kind}`,{...project,kind});
 add('PersistedWorkspaceRecord',`kind_${kind}`,{...workspace,kind});
 add('WorkspaceDescriptorPayload',`workspace_kind_${kind}`,{...descriptor,workspaceKind:kind});
 add('WorkspaceDescriptorPayload',`project_kind_${kind}`,{...descriptor,projectKind:kind});
}
for (const status of ['needs_input','failed','running','attention','done','archived']) add('WorkspaceDescriptorPayload',`status_${status}`,{...descriptor,status});
add('WorkspaceDescriptorPayload','minimal',descriptor);
add('WorkspaceScriptPayload','minimal',script);
add('WorkspaceGitHubRuntimePayload','unknown_mergeability',{pullRequest:{...pr,mergeable:'FUTURE'}});
add('WorkspaceGitHubRuntimePayload','null_mergeability',{pullRequest:{...pr,mergeable:null}});
add('WorkspaceGitHubRuntimePayload','number_mergeability',{pullRequest:{...pr,mergeable:42}});
for (const isGit of [true,false]) for (const owned of [true,false]) for (const mainRoot of [undefined,null,'/main']) for (const worktreeRoot of [undefined,null,'/tree']) {
 const input={cwd:'/cwd',isGit,currentBranch:null,remoteUrl:null,isPaseoOwnedWorktree:owned};
 if(mainRoot!==undefined)input.mainRepoRoot=mainRoot;
 if(worktreeRoot!==undefined)input.worktreeRoot=worktreeRoot;
 add('ProjectCheckoutLitePayload',`union_${isGit}_${owned}_${mainRoot}_${worktreeRoot}`,input);
}
const sourceFiles=Object.fromEntries([[recordPath,records],[messagePath,messages]].map(([name,text])=>[name,crypto.createHash('sha256').update(text).digest('hex')]));
const meta={revision,zod:require(path.join(path.resolve(zodPath),'package.json')).version,sourceFiles};
for (const [destination,isRecord] of [['crates/server-domain/tests/fixtures/paseo-registry.json',true],['crates/server-protocol/tests/fixtures/paseo-workspace.json',false]]) {
 fs.writeFileSync(destination,JSON.stringify({source:meta,cases:cases.filter(c=>c.schema.startsWith('Persisted')===isRecord)},null,2)+'\n');
}
console.log(JSON.stringify({source:meta,cases:cases.length}));
