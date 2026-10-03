"""Measure the Linux rules preview. This is not contextual-model qualification."""
import argparse
import asyncio
import json
import os
from pathlib import Path
import platform
import signal
import subprocess
import sys
import tempfile
import time
ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT / "sdk/python"))
from e2em import Client, E2EMError
from runtime_energy import EnergyReader, energy_delta

def counters(pid, proc_root=Path("/proc")):
    directory = proc_root / str(pid)
    values = {}
    for line in (directory / "status").read_text(encoding="utf-8").splitlines():
        key, _, value = line.partition(":")
        if key == "VmRSS":
            values[key] = int(value.split()[0])
    values.update(voluntary_ctxt_switches=0, nonvoluntary_ctxt_switches=0, scheduled_timeslices=0)
    threads = 0
    for task in (directory / "task").iterdir():
        try:
            status = (task / "status").read_text(encoding="utf-8")
            slices = int((task / "schedstat").read_text(encoding="utf-8").split()[2])
        except FileNotFoundError:
            continue
        threads += 1
        for line in status.splitlines():
            key, _, value = line.partition(":")
            if key in ("voluntary_ctxt_switches", "nonvoluntary_ctxt_switches"):
                values[key] += int(value.strip())
        values["scheduled_timeslices"] += slices
    fields = (directory / "stat").read_text(encoding="utf-8").rpartition(")")[2].split()
    values["cpu_seconds"] = (int(fields[11]) + int(fields[12])) / os.sysconf("SC_CLK_TCK")
    values["threads"] = threads
    return values


def idle_delta(before, after, seconds):
    return {"elapsed_seconds": seconds,
            "cpu_seconds": after["cpu_seconds"] - before["cpu_seconds"],
            "voluntary_context_switches": after["voluntary_ctxt_switches"] - before["voluntary_ctxt_switches"],
            "involuntary_context_switches": after["nonvoluntary_ctxt_switches"] - before["nonvoluntary_ctxt_switches"],
            "scheduled_timeslices": after["scheduled_timeslices"] - before["scheduled_timeslices"],
            "threads_before": before["threads"], "threads_after": after["threads"]}

def distribution(samples):
    ordered=sorted(samples)
    return {"samples":len(samples),"p50_ms":ordered[len(ordered)//2],"p95_ms":ordered[min(len(ordered)-1,int(len(ordered)*.95))]}

async def measure(args, energy=None):
    with tempfile.TemporaryDirectory() as directory:
        directory=Path(directory);directory.chmod(0o700)
        socket=directory/"runtime.sock";grants=directory/"grants.json"
        grants.write_text(json.dumps({"provider":"measurement","grants":[{"principal":"reference","uid":os.getuid(),"secret":"a"*64}]}));grants.chmod(0o600)
        process=subprocess.Popen([args.binary,"--socket",str(socket),"--grants",str(grants),"--idle-seconds","1"],stdout=subprocess.DEVNULL,stderr=subprocess.PIPE)
        try:
            for _ in range(200):
                try:
                    client=await Client.open(str(socket),"reference","a"*64,"measurement")
                    break
                except (OSError, E2EMError):
                    await asyncio.sleep(.01)
            else:
                raise RuntimeError("measurement service did not become authenticated and ready")
            async with client:
                policy=json.loads((ROOT/"tests/conformance/email-policy.json").read_text(encoding="utf-8"))
                reference=await client.validate_policy(policy)
                request=json.loads((ROOT/"tests/conformance/assessments.json").read_text(encoding="utf-8"))[0]["request"]
                request.pop("policy");request["policy_ref"]=reference
                unloaded=counters(process.pid)
                cold=[];warm=[];cycles=[]
                async def quiet_interval():
                    before = counters(process.pid)
                    first_energy = energy.sample() if energy else None
                    started = time.perf_counter()
                    await asyncio.sleep(args.idle_window_seconds)
                    last_energy = energy.sample() if energy else None
                    elapsed = time.perf_counter() - started
                    after = counters(process.pid)
                    assert (await client.capabilities())["runtime_state"] == "unloaded"
                    interval = idle_delta(before, after, elapsed)
                    interval["energy"] = energy_delta(first_energy, last_energy, elapsed) if energy else None
                    return interval
                initial_idle = await quiet_interval()
                for cycle in range(args.cycles):
                    first_energy = energy.sample() if energy else None
                    phase_started = time.perf_counter()
                    start=time.perf_counter();assert (await client.assess(request)).action == "warn";cold.append((time.perf_counter()-start)*1000)
                    for _ in range(50):
                        start=time.perf_counter();assert (await client.assess(request)).action == "warn";warm.append((time.perf_counter()-start)*1000)
                    phase_elapsed = time.perf_counter() - phase_started
                    last_energy = energy.sample() if energy else None
                    active_energy = energy_delta(first_energy, last_energy, phase_elapsed) if energy else None
                    ready=counters(process.pid)
                    await asyncio.sleep(1.1)
                    idle=await client.capabilities()
                    assert idle["runtime_state"] == "unloaded"
                    after=counters(process.pid)
                    # Connected idle capabilities must not cause a reload.
                    await asyncio.sleep(.2)
                    assert (await client.capabilities())["runtime_state"] == "unloaded"
                    cycles.append({"ready_rss_kib":ready["VmRSS"],"unloaded_rss_kib":after["VmRSS"],"idle_context_switch_delta":after["voluntary_ctxt_switches"]-ready["voluntary_ctxt_switches"], "connected_unloaded_interval":await quiet_interval(), "active_energy":active_energy})
                return {"platform":platform.system(),"kernel":platform.release(),"architecture":platform.machine(),"backend":"rules-only / pii.email","model":"none","idle_timeout_seconds":1,"configured_default_seconds":300,"initial_unloaded_rss_kib":unloaded["VmRSS"],"cycles":cycles,"cold_reload":distribution(cold),"warm_end_to_end":distribution(warm),"initial_connected_unloaded_interval":initial_idle,"energy_measurement":{"source":"Intel RAPL cumulative counters", "scope":"whole CPU domains on a shared host; overlapping domains are not summed", "counter_access":"explicit read-only offline container" if args.energy_via_container else "direct user read", "process_attribution":False} if energy else None,"limits":"No contextual model or accelerator is loaded. RSS includes Tokio threads, regex cache and allocator retention. When enabled, energy is measured for whole CPU domains, including unrelated host work and measurement overhead, and is not attributable to this process. Context switches and scheduled timeslices are wakeup proxies, not interrupt tracing. Windows/macOS measurements are unavailable. This run measures request/response latency, not a model release gate."}
        finally:
            if process.poll() is None: process.send_signal(signal.SIGINT)
            await asyncio.to_thread(process.communicate,timeout=10)

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary",required=True);parser.add_argument("--output",type=Path,required=True);parser.add_argument("--cycles",type=int,default=3)
    parser.add_argument("--idle-window-seconds", type=float, default=5)
    parser.add_argument("--energy", action="store_true", help="require direct readable RAPL counters")
    parser.add_argument("--energy-via-container", help="explicit existing Node image for read-only offline counter access")
    args=parser.parse_args()
    if not 0.1 <= args.idle_window_seconds <= 60: parser.error("idle window must be 0.1–60 seconds")
    if not 1<=args.cycles<=100:parser.error("cycles must be 1–100")
    if args.energy or args.energy_via_container:
        with EnergyReader(args.energy_via_container) as energy:
            energy.sample()  # Validate access before starting the runtime.
            report=asyncio.run(measure(args, energy))
    else:
        report=asyncio.run(measure(args))
    args.output.parent.mkdir(parents=True,exist_ok=True);args.output.write_text(json.dumps(report,indent=2)+"\n")
if __name__ == "__main__": main()
