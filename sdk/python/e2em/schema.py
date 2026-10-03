"""Validator for the exact subset emitted by our Rust contract generator."""
import json
from pathlib import Path
SCHEMA = json.loads(Path(__file__).with_name("response.schema.json").read_text(encoding="utf-8"))
def valid(value, schema=SCHEMA, root=SCHEMA):
    if "$ref" in schema: return valid(value, root["$defs"][schema["$ref"].rsplit("/",1)[1]], root)
    for key in ("anyOf", "oneOf"):
        if key in schema:
            count = sum(valid(value,s,root) for s in schema[key])
            return count == 1 if key == "oneOf" else count > 0
    if "const" in schema and value != schema["const"]: return False
    if "enum" in schema and value not in schema["enum"]: return False
    kind = schema.get("type")
    if isinstance(kind,list): return any(valid(value,{**schema,"type":k},root) for k in kind)
    if kind == "object":
        if not isinstance(value,dict) or not set(schema.get("required",[])) <= value.keys(): return False
        props = schema.get("properties",{})
        if schema.get("additionalProperties") is False and value.keys() - props.keys(): return False
        return all(valid(v,props[k],root) for k,v in value.items() if k in props)
    if kind == "array": return isinstance(value,list) and all(valid(v,schema["items"],root) for v in value)
    allowed = {"string": isinstance(value,str),"integer": type(value) is int,"number": type(value) in (int,float),"boolean": type(value) is bool,"null":value is None}
    if not allowed.get(kind,False): return False
    return type(value) not in (int,float) or "minimum" not in schema or value >= schema["minimum"]
