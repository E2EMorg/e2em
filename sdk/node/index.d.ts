import type {Request, Policy, PolicyRef, Assessment as AssessmentValue, Capabilities} from './contract.js';
export * from './contract.js';
export class E2EMError extends Error { code: string; action: 'review'; }
export interface Connection {socketPath:string; principal:string; secret:string; provider:string; model?:string;}
export interface Candidate extends Connection {kind:'native'|'project'|'embedded';}
export type AssessmentResult = AssessmentValue & {appliesTo(request:Request):boolean};
export interface AppConnection {app?:string; configPath?:string; model?:string;}
export interface MessageOptions {context?:string|string[]; policies?:string|string[]; customPolicies?:string|string[]; signal?:AbortSignal; model?:string; deadlineMs?:number;}
export type MessageAssessment = AssessmentResult & {request:Request; scores:Record<string,number>; unevaluated:string[]};
export const presets: ReadonlyArray<{readonly id:string; readonly tier:string; readonly wording:string; readonly directions:ReadonlyArray<string>}>;
export function assess(message:string, options?:MessageOptions & AppConnection):Promise<MessageAssessment>;
export function utf16Span(text:string,start:number,end:number):[number,number];
export class Client {
 static open(connection:Connection):Promise<Client>;
 static connect(options?:AppConnection):Promise<Client>;
 static discover(candidates:Candidate[],requiredDetectors:string[],pinned?:string):Promise<Client>;
 provider:string; capabilityManifest:Capabilities;
 capabilities():Promise<Capabilities>;
 validatePolicy(policy:Policy):Promise<PolicyRef>;
 models():Promise<import('./contract.js').ReplyModels>;
 installModel(source:string,options?:{name?:string;autoUpdate?:boolean}):Promise<import('./contract.js').ReplyModels>;
 assess(request:Request,options?:{signal?:AbortSignal}):Promise<AssessmentResult>;
 assess(message:string,options?:MessageOptions):Promise<MessageAssessment>;
 cancel(requestId:string):Promise<boolean>;
 close():void;
}
