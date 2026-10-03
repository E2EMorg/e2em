import type {Request, Policy, PolicyRef, Assessment as AssessmentValue, Capabilities} from './contract.js';
export * from './contract.js';
export class E2EMError extends Error { code: string; action: 'review'; }
export interface Connection {socketPath:string; principal:string; secret:string; provider:string;}
export interface Candidate extends Connection {kind:'native'|'project'|'embedded';}
export type AssessmentResult = AssessmentValue & {appliesTo(request:Request):boolean};
export function utf16Span(text:string,start:number,end:number):[number,number];
export class Client {
 static open(connection:Connection):Promise<Client>;
 static discover(candidates:Candidate[],requiredDetectors:string[],pinned?:string):Promise<Client>;
 provider:string; capabilityManifest:Capabilities;
 capabilities():Promise<Capabilities>;
 validatePolicy(policy:Policy):Promise<PolicyRef>;
 assess(request:Request,options?:{signal?:AbortSignal}):Promise<AssessmentResult>;
 cancel(requestId:string):Promise<boolean>;
 close():void;
}
