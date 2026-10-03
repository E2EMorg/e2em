"""Reference local outgoing hook. Credentials are provisioned before assessment."""
import asyncio
import copy
import json
from pathlib import Path
import sys
from e2em import Client, E2EMError

async def main():
    candidates=json.loads(Path(sys.argv[1]).read_text())
    policy=json.loads(Path(sys.argv[2]).read_text())
    client=await Client.discover(candidates,["pii.email"])
    async with client:
        reference=await client.validate_policy(policy)
        current={"api_version":"0.1","request_id":"draft-1","direction":"outgoing","message":{"id":"draft","revision":"1","speaker":"self","text":input("Draft: ")},"policy_ref":reference}
        snapshot=copy.deepcopy(current)
        try: result=await client.assess(snapshot)
        except E2EMError as error:
            print(f"Message held: {error.code}. Edit or retry.")
            return
        # The real host increments revision and changes current when an edit occurs.
        if not result.applies_to(current):
            print("Draft changed; assess its new revision.");return
        permitted=result.action == "allow"
        if result.action == "warn" and policy["override"] == "user_confirm":
            permitted=input("This message includes an email address. Type send to continue: ") == "send"
        if permitted and result.applies_to(current):
            print("Local reference send operation accepted.")
        else: print("Message held. Edit or retry.")
if __name__ == "__main__": asyncio.run(main())
