import fs from 'node:fs/promises';
import readline from 'node:readline/promises';
import {Client} from '../index.js';
const candidates=JSON.parse(await fs.readFile(process.argv[2],'utf8'));
const policy=JSON.parse(await fs.readFile(process.argv[3],'utf8'));
const input=readline.createInterface({input:process.stdin,output:process.stdout});
const client=await Client.discover(candidates,[]);
try {
 const reference=await client.validatePolicy(policy);
 const current={api_version:'0.1',request_id:'draft-1',direction:'outgoing',message:{id:'draft',revision:'1',speaker:'self',text:await input.question('Chat message: ')},policy_ref:reference};
 const result=await client.assess(current);
 for (const finding of result.findings) if (finding.score !== null) console.log(`Category ${finding.category}: score ${finding.score}`);
 if (result.coverage.unevaluated_rules.length) console.log(`Unevaluated rules: ${result.coverage.unevaluated_rules.join(', ')} (${result.reason_codes.join(', ')})`);
 if (!result.appliesTo(current)) throw new Error('stale revision');
 let permitted=result.action === 'allow';
 if (result.action === 'warn' && policy.override === 'user_confirm') permitted=await input.question('This chat message matched your policy. Type send to continue: ') === 'send';
 console.log(permitted && result.appliesTo(current) ? 'Message accepted by the local reference chat.' : 'Message held. Edit or retry.');
} catch(error) { console.log(`Message held: ${error.code ?? 'INTERNAL_ERROR'}. Edit or retry.`); }
finally {client.close();input.close();}
