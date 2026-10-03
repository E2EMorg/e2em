import {Client, assess, type Request, type Policy, type AssessmentResult, type MessageAssessment} from './index.js';
export async function integration(client:Client, request:Request, policy:Policy):Promise<AssessmentResult> {
 const reference = await client.validatePolicy(policy);
 const next:Request = {...request, policy:undefined, policy_ref:reference};
 const assessment = await client.assess(next);
 const valid:boolean = assessment.appliesTo(next);
 if (!valid) throw new Error('stale');
 return assessment;
}
// @ts-expect-error unsupported direction
const invalid:Request['direction'] = 'sideways';
// @ts-expect-error scalar results cannot masquerade as complete assessments
const scalar:AssessmentResult = {score:0.9};
void invalid; void scalar;
export async function messageIntegration(client:Client):Promise<MessageAssessment> {
 const defaults = await client.assess('Hello', {context:['Earlier message']});
 const custom = await client.assess('Hello', {policies:[],customPolicies:['Keep project details private.']});
 const score:number|undefined = custom.scores['custom-1'];
 void score;
 defaults.appliesTo(defaults.request);
 return assess('Hello');
}
