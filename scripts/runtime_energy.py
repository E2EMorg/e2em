"""Optional RAPL snapshots. Container access is explicit, read-only and offline."""
import json
from pathlib import Path
import select
import subprocess
import uuid

ROOT = Path("/sys/devices/virtual/powercap/intel-rapl")


def read_domains(root=ROOT):
    domains = {}
    for energy in sorted(root.glob("**/energy_uj")):
        folder = energy.parent
        domains[folder.name] = {"name": (folder / "name").read_text().strip(),
                               "energy_uj": int(energy.read_text()),
                               "max_energy_range_uj": int((folder / "max_energy_range_uj").read_text())}
    if not domains:
        raise OSError("no RAPL energy domains found")
    return domains


def energy_delta(before, after, seconds):
    if seconds <= 0 or before.keys() != after.keys():
        raise ValueError("energy samples require matching domains and positive elapsed time")
    result = {}
    for key, first in before.items():
        last = after[key]
        maximum = first["max_energy_range_uj"]
        if maximum <= 0 or maximum != last["max_energy_range_uj"] or first["name"] != last["name"]:
            raise ValueError("energy domain changed during measurement")
        if not 0 <= first["energy_uj"] < maximum or not 0 <= last["energy_uj"] < maximum:
            raise ValueError("energy counter outside its documented range")
        joules = ((last["energy_uj"] - first["energy_uj"]) % maximum) / 1_000_000
        result[key] = {"name": first["name"], "joules": joules,
                       "average_watts": joules / seconds}
    return {"elapsed_seconds": seconds, "domains": result}


class EnergyReader:
    def __init__(self, container_image=None):
        self.process = None
        self.name = None
        if container_image:
            self.name = "e2em-energy-" + uuid.uuid4().hex
            program = r'''
const fs = require('node:fs'), readline = require('node:readline');
const root = '/powercap/intel-rapl';
function domains(folder, output) {
  for (const entry of fs.readdirSync(folder, {withFileTypes:true})) {
    if (entry.isDirectory()) domains(folder + '/' + entry.name, output);
  }
  if (fs.existsSync(folder + '/energy_uj')) {
    output[folder.split('/').at(-1)] = {
      name:fs.readFileSync(folder + '/name','utf8').trim(),
      energy_uj:Number(fs.readFileSync(folder + '/energy_uj','utf8')),
      max_energy_range_uj:Number(fs.readFileSync(folder + '/max_energy_range_uj','utf8'))
    };
  }
}
readline.createInterface({input:process.stdin,terminal:false}).on('line',()=>{
  try { const result={}; domains(root,result); console.log(JSON.stringify(result)); }
  catch { console.log(JSON.stringify({error:'RAPL counters unavailable'})); }
});
'''
            self.process = subprocess.Popen([
                "docker", "run", "--rm", "--pull", "never", "--name", self.name,
                "--network", "none", "--read-only", "-i", "--mount",
                f"type=bind,src={ROOT.parent},dst=/powercap,readonly",
                container_image, "node", "-e", program],
                stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)

    def sample(self):
        if self.process is None:
            return read_domains()
        self.process.stdin.write("snapshot\n")
        self.process.stdin.flush()
        if not select.select([self.process.stdout], [], [], 10)[0]:
            raise TimeoutError("energy reader did not respond")
        line = self.process.stdout.readline(65_537)
        if not line or len(line) > 65_536:
            raise OSError("invalid energy-reader response")
        value = json.loads(line)
        if not value or "error" in value:
            raise OSError("RAPL counters unavailable")
        return value

    def close(self):
        if self.process is not None:
            self.process.stdin.close()
            self.process.stdin = None
            try:
                self.process.communicate(timeout=5)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.communicate()
            finally:
                subprocess.run(["docker", "rm", "-f", self.name], capture_output=True, check=False)
            self.process = None

    def __enter__(self):
        return self

    def __exit__(self, *_):
        self.close()
