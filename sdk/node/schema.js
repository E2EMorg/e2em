import schema from './response.schema.json' with {type:'json'};
export function valid(value, rule=schema, root=schema) {
  if (rule.$ref) return valid(value,root.$defs[rule.$ref.split('/').at(-1)],root);
  for (const key of ['anyOf','oneOf']) if (rule[key]) {
    const count = rule[key].filter(r=>valid(value,r,root)).length; return key === 'oneOf' ? count === 1 : count > 0;
  }
  if ('const' in rule && value !== rule.const) return false;
  if (rule.enum && !rule.enum.includes(value)) return false;
  const kind = rule.type;
  if (Array.isArray(kind)) return kind.some(k=>valid(value,{...rule,type:k},root));
  if (kind === 'object') {
    if (!value || typeof value !== 'object' || Array.isArray(value) || (rule.required ?? []).some(k=>!(k in value))) return false;
    const props = rule.properties ?? {};
    if (rule.additionalProperties === false && Object.keys(value).some(k=>!(k in props))) return false;
    return Object.entries(value).every(([k,v])=>!(k in props) || valid(v,props[k],root));
  }
  if (kind === 'array') return Array.isArray(value) && value.every(v=>valid(v,rule.items,root));
  const allowed = {string:typeof value === 'string',integer:Number.isSafeInteger(value),number:typeof value === 'number' && Number.isFinite(value),boolean:typeof value === 'boolean',null:value === null};
  return !!allowed[kind] && (!('minimum' in rule) || value >= rule.minimum);
}
