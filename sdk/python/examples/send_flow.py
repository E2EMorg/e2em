"""Reference local chat send hook. Credentials are provisioned before assessment."""
import asyncio
import copy
import json
from pathlib import Path
import sys
from e2em import Client, E2EMError

async def main():
    candidates=json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
    policy=json.loads(Path(sys.argv[2]).read_text(encoding="utf-8"))
    client=await Client.discover(candidates,[])
    async with client:
        reference=await client.validate_policy(policy)
        current={"api_version":"0.1","request_id":"draft-1","direction":"outgoing","message":{"id":"draft","revision":"1","speaker":"self","text":input("Chat message: ")},"policy_ref":reference}
        snapshot=copy.deepcopy(current)
        try: result=await client.assess(snapshot)
        except E2EMError as error:
            print(f"Message held: {error.code}. Edit or retry.")
            return
        for finding in result.value["findings"]:
            if finding["score"] is not None:
                print(f"Category {finding['category']}: score {finding['score']}")
        if result.value["coverage"]["unevaluated_rules"]:
            print("Unevaluated rules:", ", ".join(result.value["coverage"]["unevaluated_rules"]),
                  "(" + ", ".join(result.value["reason_codes"]) + ")")
        # The real host increments revision and changes current when an edit occurs.
        if not result.applies_to(current):
            print("Draft changed; assess its new revision.");return
        permitted=result.status == "assessed" and result.action in ("allow", "warn")
        if result.action == "warn":
            print("Warning: this chat message matched your policy.")
        if permitted and result.applies_to(current):
            print("Message accepted by the local reference chat.")
        else: print("Message held. Edit or retry.")
if __name__ == "__main__": asyncio.run(main())
