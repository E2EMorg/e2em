import {Client, type Request, type Policy, type AssessmentResult} from './index.js';
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
