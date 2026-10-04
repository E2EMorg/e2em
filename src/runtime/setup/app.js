const $ = id => document.getElementById(id);
let stage = 'welcome', preferencesLoaded = false, closed = false;
const busyStages = ['configuring', 'model', 'starting', 'checking'];
const requestedApp = new URLSearchParams(location.search).get('app');
if (requestedApp && /^[A-Za-z0-9_-]{1,64}$/.test(requestedApp)) $('principal').value = requestedApp;
async function post(path, body) {
  const response = await fetch(path, {method:'POST', headers:{'Content-Type':'application/json'}, body:JSON.stringify(body)});
  const text = await response.text();
  let value; try { value = JSON.parse(text); } catch { value = {message:text}; }
  if (!response.ok) throw new Error(value.message || text);
  return value;
}
function render(state) {
  stage = state.stage;
  const busy = busyStages.includes(stage), ready = stage === 'ready';
  $('status-title').textContent = ready ? 'Ready' : stage === 'error' ? 'Let’s finish setup' : busy ? 'Setting up E2EM' : 'Welcome';
  $('message').textContent = state.message;
  $('indicator').className = ready ? 'ready' : busy ? 'busy' : stage === 'error' ? 'error' : '';
  $('setup-form').hidden = ready;
  $('connect').hidden = !ready;
  $('steps').hidden = ready;
  $('start').disabled = busy;
  $('start').textContent = busy ? 'Setup in progress…' : stage === 'error' ? 'Try again' : 'Set up E2EM';
  for (const id of ['login', 'updates', 'offline']) $(id).disabled = busy;
  document.querySelectorAll('[data-step]').forEach((item, index) => {
    const current = busyStages.indexOf(stage);
    item.className = index < current ? 'complete' : index === current ? 'active' : '';
  });
  if (state.preferences && !preferencesLoaded) {
    $('login').checked = state.preferences.start_at_login;
    $('updates').checked = state.preferences.auto_update;
    $('offline').checked = state.preferences.offline;
    preferencesLoaded = true;
  }
}
async function poll() {
  if (closed) return;
  try {
    const response = await fetch('status');
    if (!response.ok) throw new Error('Setup status is unavailable.');
    render(await response.json());
  } catch {
    $('message').textContent = 'The setup connection closed. Reopen E2EM Setup to check progress or continue. Completed steps are kept.';
    $('start').disabled = true;
  }
  if (!closed) setTimeout(poll, 1500);
}
$('offline').addEventListener('change', () => { if ($('offline').checked) $('updates').checked = false; });
$('setup-form').addEventListener('submit', async event => {
  event.preventDefault();
  $('start').disabled = true;
  try {
    await post('start', {offline:$('offline').checked, auto_update:$('updates').checked && !$('offline').checked, start_at_login:$('login').checked});
    render({stage:'configuring', message:'Preparing your private runtime…'});
  } catch (error) { render({stage:'error', message:error.message}); }
});
$('app-form').addEventListener('submit', async event => {
  event.preventDefault();
  const button = event.target.querySelector('button'); button.disabled = true;
  try {
    const result = await post('enrol', {principal:$('principal').value});
    $('app-message').textContent = result.message;
  } catch (error) { $('app-message').textContent = error.message; }
  finally { button.disabled = false; }
});
$('done').addEventListener('click', async () => {
  try { await post('close', {}); closed = true; $('done').disabled = true; $('done').textContent = 'All done'; $('message').textContent = 'E2EM keeps running in the background. You can close this tab.'; }
  catch (error) { $('message').textContent = error.message; }
});
poll();
