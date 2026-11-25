'use strict';

const screen = document.querySelector('#screen');
const nav = document.querySelector('nav');
const waiting = new Map();
let requestId = 0;
let current = 'start';
let fictional = false;
let cleanups = [];
let recordingPath = '';
let earlierRecordings = [];
let recent = [];
let caseFile = '';
let nextLine = null;
let storySelection;
try {
  const saved = JSON.parse(localStorage.getItem('parsley-reports') || '[]');
  if (Array.isArray(saved)) recent = saved.filter(item => typeof item?.path === 'string' &&
    typeof item.label === 'string' && /\.html?$/i.test(item.path)).slice(0, 8);
} catch {}
try {
  const paths = JSON.parse(localStorage.getItem('parsley-recordings') || '[]');
  if (Array.isArray(paths)) earlierRecordings = paths.filter(path => typeof path === 'string' && /\.jsonl$/i.test(path)).slice(0, 8);
  recordingPath = earlierRecordings[0] || '';
} catch {}
document.querySelector('.skip').addEventListener('click', () => screen.focus());

window.parsleyReply = ({ id, value, error }) => {
  const pending = waiting.get(id);
  if (!pending) return;
  waiting.delete(id);
  clearTimeout(pending.timer);
  if (error) pending.reject(new Error(error));
  else pending.resolve(value);
};

function api(action, data = {}) {
  return new Promise((resolve, reject) => {
    const id = ++requestId;
    const timer = setTimeout(() => {
      waiting.delete(id);
      reject(new Error('Parsley did not receive a response. Try again.'));
    }, action === 'pick' || action === 'bandwidth' ? 180000 : 90000);
    waiting.set(id, { resolve, reject, timer });
    if (!window.ipc) {
      window.parsleyReply({ id, error: 'Open this screen from Parsley Desktop.' });
      return;
    }
    window.ipc.postMessage(JSON.stringify({ id, action, data }));
  });
}

function el(tag, cls, ...children) {
  const element = document.createElement(tag);
  if (cls) element.className = cls;
  element.append(...children.flat().filter(child => child !== undefined && child !== null));
  return element;
}
const p = text => el('p', '', text);
const row = (...children) => el('div', 'row', ...children);
const actions = (...children) => el('div', 'actions', ...children);
const card = (...children) => el('section', 'card', ...children);
const stack = (...children) => el('div', 'stack', ...children);
const title = text => el('h2', '', text);
const num = value => typeof value === 'number' ? value.toLocaleString('en-AU') : String(value ?? '—');
const displayTime = at => new Date(at).toLocaleString('en-AU', { dateStyle: 'medium', timeStyle: 'medium' });
const msg = () => {
  const box = el('div', 'banner');
  box.setAttribute('role', 'status');
  return box;
};
function tell(box, text, tone = '') {
  box.textContent = text;
  box.className = 'banner' + (tone ? ' ' + tone : '');
}
function button(text, handler, cls = '') {
  const control = el('button', cls, text);
  control.type = 'button';
  control.addEventListener('click', handler);
  return control;
}
function work(text, box, handler, cls = '') {
  return button(text, async event => {
    const control = event.currentTarget;
    if (control.disabled) return;
    control.disabled = true;
    control.setAttribute('aria-busy', 'true');
    try { await handler(); }
    catch (error) { tell(box, error.message, /cancel/i.test(error.message) ? 'warn' : 'error'); }
    finally { control.disabled = false; control.removeAttribute('aria-busy'); }
  }, cls);
}
function field(label, type = 'text', value = '') {
  const control = el('input');
  control.type = type;
  control.value = value;
  return { control, view: el('label', '', label, control), value: () => control.value };
}
function select(label, options) {
  const control = el('select', 'select');
  for (const [value, text] of options) {
    const option = el('option', '', text);
    option.value = value;
    control.append(option);
  }
  return { control, view: el('label', '', label, control), value: () => control.value };
}
function check(text) {
  const control = el('input');
  control.type = 'checkbox';
  return { control, view: el('label', 'check', control, text), value: () => control.checked };
}
function file(label, kind, status) {
  let path = '';
  const chosen = el('span', '', 'No file selected');
  const clear = button('Clear', () => {
    path = '';
    chosen.textContent = 'No file selected';
    chosen.title = '';
    clear.hidden = true;
  }, 'link');
  clear.hidden = true;
  const choose = work('Browse…', status, async () => {
    const next = await api('pick', { kind });
    if (!next) return;
    path = next;
    chosen.textContent = next.split(/[\\/]/).pop();
    chosen.title = next;
    clear.hidden = false;
  });
  return { view: el('div', 'stack', el('strong', '', label), el('div', 'file', choose, chosen, clear)), value: () => path };
}
function date(field) {
  const value = field.value();
  if (!value) return '';
  const result = new Date(value);
  if (!Number.isFinite(result.valueOf())) throw new Error('Choose a valid date and time.');
  return result.toISOString();
}
function heading(label, name, subtitle, nextText) {
  nextLine = nextText ? el('div', 'next', el('b', '', 'Next'), el('span', '', nextText)) : null;
  if (nextLine) nextLine.setAttribute('aria-live', 'polite');
  return el('header', 'heading',
    el('div', 'kicker', label),
    el('h1', '', name),
    subtitle ? p(subtitle) : null,
    nextLine);
}
function next(text) {
  if (nextLine) nextLine.lastChild.textContent = text;
}
function emptyState(text) {
  const mascot = el('img');
  mascot.src = '/bunch.svg';
  mascot.alt = '';
  return el('div', 'empty', mascot, el('span', '', text));
}
const cancelled = status => tell(status, 'Save cancelled. Nothing was written.');
function table(headers, rows) {
  const head = el('tr', '', headers.map(name => {
    const cell = el('th', '', name);
    cell.scope = 'col';
    return cell;
  }));
  return el('div', 'table-wrap', el('table', '',
    el('thead', '', head), el('tbody', '', rows.map(values => el('tr', '', values.map(value => el('td', '', value)))))));
}
function preview(target, report) {
  const frame = el('iframe');
  frame.title = 'Local report preview';
  frame.setAttribute('sandbox', '');
  frame.srcdoc = report.html.replace('</head>', '<style>#print-report,.filters,.official-help{display:none}</style></head>');
  target.replaceChildren(p('Save and open the report to follow help links, search or print.'), frame);
}
function saveRecent(path, label) {
  if (!/\.html?$/i.test(path)) return;
  recent = [{ path, label }, ...recent.filter(item => item.path !== path)].slice(0, 8);
  try { localStorage.setItem('parsley-reports', JSON.stringify(recent)); } catch {}
}
function showSavedReport(status, path, label) {
  saveRecent(path, label);
  tell(status, '');
  status.append(p('Saved to ' + path), p('Find it on Home under Reopen a local report.'),
    actions(work('Open report', status, () => localOpen(path, status))));
  next('Open the report now, or come back to it from Home.');
}
async function saveReport(kind, data, format, status, label) {
  const demo = fictional;
  const path = await api('pick', { kind: format });
  if (!path) return cancelled(status);
  tell(status, 'Saving report…', 'busy');
  try { await api('report', { ...data, kind, path, demo }); }
  catch (error) { throw new Error('Could not save the report. ' + error.message + ' Existing files were not changed.'); }
  if (/\.html?$/i.test(path)) {
    showSavedReport(status, path, demo ? 'Practice · ' + label : label);
  } else {
    tell(status, 'Saved to ' + path);
  }
}
function localOpen(path, status) {
  return api('open', { path }).then(() => tell(status, 'Opened the local report.'));
}
function interval(fn, time) {
  const timer = setInterval(fn, time);
  cleanups.push(() => clearInterval(timer));
}
const pages = { start, rewind, events: crashes, find, compare, health, network, inventory, help, export: exports };
const sections = [
  ['Home', 'home', [['start', 'Home']]],
  ['What happened', 'history', [['rewind', 'Story'], ['find', 'All events'], ['events', 'Crashes'], ['compare', 'Compare']]],
  ['Live', 'live', [['health', 'Health'], ['network', 'Network']]],
  ['My PC', 'pc', [['inventory', 'My PC']]],
  ['How to and help', 'help', [['help', 'How to and report'], ['export', 'Export']]],
];
const icons = {
  home: 'M3 10l9-7 9 7v10H3z M9 20v-7h6v7',
  history: 'M3 11a9 9 0 1 1 2.5 7 M3 4v7h7 M12 7v5l3 2',
  live: 'M2 12h4l3-7 4 14 3-7h6',
  pc: 'M3 4h18v12H3z M12 16v4 M8 20h8',
  help: 'M9 9a3 3 0 1 1 4.5 2.6c-1 .6-1.5 1.4-1.5 2.4 M12 17h.01 M22 12a10 10 0 1 1-20 0 10 10 0 0 1 20 0',
};
function navIcon(name) {
  const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  svg.setAttribute('viewBox', '0 0 24 24');
  svg.setAttribute('aria-hidden', 'true');
  const path = document.createElementNS(svg.namespaceURI, 'path');
  path.setAttribute('d', icons[name]);
  svg.append(path);
  return svg;
}
nav.addEventListener('keydown', event => {
  if (!['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key)) return;
  const links = [...nav.querySelectorAll('button')];
  const index = links.indexOf(document.activeElement);
  if (index < 0) return;
  event.preventDefault();
  const target = event.key === 'Home' ? 0 : event.key === 'End' ? links.length - 1
    : (index + (event.key === 'ArrowDown' ? 1 : -1) + links.length) % links.length;
  links[target].focus();
});

function go(key, demo = false, options) {
  for (const cleanup of cleanups) cleanup();
  cleanups = [];
  current = key;
  fictional = demo;
  const active = sections.find(([, , views]) => views.some(([id]) => id === key));
  const views = active[2];
  const label = views.find(([id]) => id === key)[1];
  nav.replaceChildren(...sections.map(section => {
    const [name, icon, routes] = section;
    const link = button('', () => go(routes[0][0]));
    link.append(navIcon(icon), name);
    if (section === active) link.setAttribute('aria-current', 'page');
    return link;
  }));
  const tabs = views.length > 1 ? el('nav', 'tabs', views.map(([id, name]) => {
    const link = button(name, () => go(id, fictional, id === 'rewind' && fictional ? { caseId: storySelection?.data.case } : undefined));
    if (id === key) link.setAttribute('aria-current', 'page');
    return link;
  })) : null;
  if (tabs) tabs.setAttribute('aria-label', active[0] + ' views');
  const mode = fictional ? el('div', 'row practice-mode', el('span', 'badge warn', 'Practice mode · Made-up data'),
    button('Exit practice', () => go('help'), 'link')) : null;
  const content = pages[key](options);
  content.classList.add('pz-in');
  screen.replaceChildren(...[tabs, mode, content].filter(Boolean));
  screen.setAttribute('aria-label', label);
  document.title = label + ' · Parsley';
  screen.focus({ preventScroll: true });
  window.scrollTo(0, 0);
}

function start() {
  const status = msg();
  const tile = (icon, label, text, ...links) => el('section', 'card tile', el('div', 'tile-icon', navIcon(icon)), title(label), p(text), actions(...links));
  const link = (text, target) => button(text + ' →', () => go(target), 'link');
  const mascot = el('img', 'mascot');
  mascot.src = '/bunch.svg';
  mascot.alt = '';
  const browse = work('Browse…', status, async () => {
    const path = await api('pick', { kind: 'open-report' });
    if (path) await localOpen(path, status);
  });
  const reports = card(el('div', 'row between', title('Reopen a local report'), browse),
    recent.length ? table(['Report', 'File', ''], recent.map(item => [item.label, item.path.split(/[\\/]/).pop(),
      work('Open', status, () => localOpen(item.path, status), 'link')])) : emptyState('Reports you save appear here. Browse for one saved earlier.'), status);
  const summary = msg();
  if (!window.parsleySmoke) {
    tell(summary, 'Checking recent Windows records…', 'busy');
    api('events').then(data => {
      const groups = Object.entries(data.counts).sort((a, b) => b[1] - a[1]).slice(0, 3);
      tell(summary, num(data.count) + (data.count === 1 ? ' supported Windows record' : ' supported Windows records') + ' in the last 24 hours.');
      if (groups.length) summary.append(el('ul', 'home-counts', groups.map(([name, count]) => el('li', '', name + ' · ' + num(count)))));
      if (data.notices.length) summary.append(p('Some Windows records could be missing. Open Story for details.'));
    }).catch(error => tell(summary, error.message, 'error'));
  }
  return el('div', '',
    heading('Home', 'Your PC, at a glance', 'See what Windows recorded and decide what to check next. Everything stays on this PC.',
      'If the same problem keeps coming back, open Crashes.'),
    el('div', 'home-grid',
      el('section', 'card hero', el('div', '',
      title('Find out what happened'),
      p('Start with the last 24 hours, or choose the time your PC started acting up.'),
      actions(button('Check the last 24 hours', () => go('rewind', false, { scanLatest: true }), 'light'),
        button('Choose a time', () => go('rewind', false, { chooseTime: true }), 'ghost'),
        button('How to use Parsley', () => go('help'), 'ghost'))), mascot),
      card(title('What Windows recorded'), summary, actions(link('All events', 'find')))),
    recent.length ? reports : null,
    el('div', 'cards home-shortcuts',
      tile('history', 'Keeps crashing?', 'Find repeat crashes and restarts.', link('Crashes', 'events')),
      tile('live', 'Running slowly?', 'See which apps are using your PC’s memory.', link('Live health', 'health')),
      tile('pc', 'What changed?', 'Compare two times or check what’s installed.', link('Compare', 'compare'), link('My PC', 'inventory')),
      tile('help', 'Need a hand?', 'Make a report for someone helping you.', link('How to and help', 'help'))),
    recent.length ? null : reports);
}

function help() {
  const cases = actions();
  cases.setAttribute('role', 'group');
  cases.setAttribute('aria-label', 'Practice cases');
  const status = msg();
  const practice = el('details', '', el('summary', '', 'Practice with example data'),
    p('These cases use made-up events. Choose a case to see how Rewind works.'), cases, status);
  let loading = false;
  practice.addEventListener('toggle', async () => {
    if (!practice.open || loading || cases.hasChildNodes()) return;
    loading = true;
    tell(status, 'Loading practice cases…', 'busy');
    try {
      const items = await api('demo-cases');
      cases.replaceChildren(...items.map(item => button(item.title, () => go('rewind', true, { caseId: item.id }))));
      tell(status, '');
    } catch (error) {
      tell(status, error.message, 'error');
    } finally {
      loading = false;
    }
  });
  return el('div', '', heading('How to', 'How to use Parsley', 'Start with what you noticed, such as a restart, a crash or a slowdown.'),
    el('div', 'split', stack(card(title('Find out what happened on this PC'),
      el('ol', '',
        el('li', '', 'On Home, check the last 24 hours or choose when the problem happened.'),
        el('li', '', 'Select an item in the timeline to see what Windows recorded.'),
        el('li', '', 'Read the next check and open its evidence.'),
        el('li', '', 'Use Save report to keep a copy. Open it later from Home.')),
      actions(button('Go to Home', () => go('start'), 'green'))),
    card(title('See what your PC is doing now'),
      p('Live health shows readings from this PC. Start monitoring if you want to watch them change.'),
      actions(button('Open Live health', () => go('health')))),
    card(practice)), supportReports()));
}

function eventTable(data, selectEvent, bookmark) {
  if (!data.rows.length) return emptyState('No matching Windows records. Widen the time range or try another log.');
  return table(['Time', 'Event', 'Source', 'Record', ...(bookmark ? ['Save'] : [])], data.rows.map(event => [
    displayTime(event.at), button(event.title, () => selectEvent(event), 'link event-select'),
    event.provider, el('span', 'numeric', event.record), ...(bookmark ? [bookmark(event)] : []),
  ]));
}
function eventStory(data, selectEvent) {
  if (!data.rows.length) return emptyState('No supported records were found in this window.');
  return el('ol', 'story-list', data.rows.map(event => {
    const item = el('li', '', el('time', 'numeric', displayTime(event.at)),
      button(event.title, () => selectEvent(event), 'link'), p(event.explanation));
    item.dataset.record = event.record;
    item.dataset.kind = event.kind;
    return item;
  }));
}
function nextStep(step, help, status) {
  if (!step) return null;
  return el('div', 'event-next', el('strong', '', 'Next check'), p(step), helpLink(help, status));
}

function helpLink(help, status) {
  return help ? work(help.label + ' ↗', status, () => api('open-help', { topic: help.id }), 'link') : null;
}
function eventDetails(box, event, status, source = '') {
  const demo = fictional;
  const raw = el('pre');
  const tools = [];
  if (!demo) {
    if (!source && Number.isSafeInteger(Number(event.record))) {
      tools.push(work('Bookmark', status, async () => {
        await api('bookmark-save', { bookmark: { channel: event.channel, record: Number(event.record), title: event.title, time_utc: event.at } });
        tell(status, 'Windows record bookmarked.');
      }));
    }
    if (caseFile && Number.isSafeInteger(Number(event.record))) {
      tools.push(work('Add to incident case', status, async () => {
        const path = await api('pick', { kind: 'json' });
        if (!path) return tell(status, 'Save cancelled. The case was not changed.');
        tell(status, 'Adding the record to a new copy of the case…', 'busy');
        try { tell(status, await api('case-bookmark', { input: caseFile, path, channel: event.channel, record: String(event.record) })); }
        catch (error) { throw new Error('Could not save the case. ' + error.message); }
        setCase(path);
      }));
    }
    tools.push(work('Show raw XML', status, async () => {
      raw.textContent = await api('evidence', { channel: event.channel, record: event.record, expected: event.at, source });
    }));
  }
  box.replaceChildren(...[title(event.title), p(event.explanation),
    event.application ? p('Application: ' + event.application) : null,
    typeof event.stop_code === 'number' ? p('Stop code: 0x' + event.stop_code.toString(16).toUpperCase()) : null,
    demo ? null : nextStep(event.next_step, event.help, status),
    el('details', 'record-details', el('summary', '', 'Windows record details'),
      p(displayTime(event.at) + ' · ' + event.channel + ' · ' + event.provider + ' · Event ' + event.id + ' · Record ' + event.record),
      event.module ? p('Faulting component: ' + event.module) : null,
      demo ? null : el('p', 'privacy', 'Raw records can contain private details.'),
      tools.length ? actions(...tools) : null, raw)].filter(Boolean));
}
function eventsResult(data, view, details, status, source, story = false, bookmark) {
  const rows = new Map();
  const selectRecord = event => {
    for (const [record, control] of rows) {
      const selected = record === event;
      control.closest('li, tr').classList.toggle('selected', selected);
      if (selected) control.setAttribute('aria-current', 'true');
      else control.removeAttribute('aria-current');
    }
    eventDetails(details, event, status, source);
  };
  view.replaceChildren(
    el('div', 'row between', title(story ? 'The timeline' : 'Windows records'), el('span', 'badge', num(data.count) + ' records')),
    story ? eventStory(data, selectRecord) : eventTable(data, selectRecord, bookmark));
  const controls = view.querySelectorAll(story ? '.story-list button' : 'tbody .event-select');
  data.rows.forEach((event, index) => rows.set(event, controls[index]));
  if (data.rows.length) eventDetails(details, data.rows[0], status, source);
  else details.replaceChildren(title('Event details'), p('Select a record to see its explanation.'));
  const notices = data.notices.slice();
  if (data.count > data.rows.length) notices.push('Showing the newest ' + data.rows.length + ' records.');
  tell(status, notices.join('\n'), notices.length ? 'warn' : '');
  return selectRecord;
}

function rewind({ caseId, scanLatest, chooseTime } = {}) {
  const demo = fictional;
  const status = msg();
  const range = select('When did it happen?', [['day', 'Last 24 hours'], ['at', 'Choose a date and time'], ['file', 'Open a Windows log file (.evtx)']]);
  if (chooseTime) range.control.value = 'at';
  const at = field('When the problem happened', 'datetime-local');
  const span = select('Time on each side', [
    ['10', '10 minutes'], ['30', '30 minutes'], ['120', '2 hours'], ['custom', 'Choose a length'],
  ]);
  const minutes = field('Minutes on each side (1–180)', 'number', '10');
  minutes.control.min = '1'; minutes.control.max = '180';
  minutes.view.hidden = true;
  span.control.addEventListener('change', () => { minutes.view.hidden = span.value() !== 'custom'; });
  const radius = () => span.value() === 'custom' ? minutes.value() : span.value();
  const source = file('Windows event log', 'event', status);
  const input = el('div', 'fieldrow', at.view, span.view, minutes.view);
  const view = card(title('Timeline'), p('Scan a time range or choose an exported Windows log.'));
  const details = card(title('Event details'), p('Select a record to see its explanation.'));
  const picker = select('Practice case', [['', 'Loading cases…']]);
  const caseNotes = el('div', 'stack');
  const header = heading('What happened', demo ? 'Practise finding evidence' : 'What happened on this PC?',
    demo ? 'Choose a made-up case and follow its next check.' : 'See what Windows recorded around the time of the problem.',
    demo ? 'Pick an example to get started.' : 'Choose a time, then check Windows records.');
  let scanId = 0;
  const config = () => {
    if (demo) {
      if (!picker.value()) throw new Error('Choose a fictional case.');
      return { case: picker.value() };
    }
    if (range.value() === 'file') {
      if (!source.value()) throw new Error('Choose an .evtx file.');
      return date(at) ? { source: source.value(), at: date(at), minutes: radius() } : { source: source.value() };
    }
    if (range.value() === 'at') {
      if (!date(at)) throw new Error('Choose the incident date and time.');
      return { at: date(at), minutes: radius() };
    }
    return {};
  };
  const scan = async () => {
    const id = ++scanId;
    if (demo) caseNotes.replaceChildren();
    tell(status, demo ? 'Loading the fictional case…' : 'Reading Windows event records…', 'busy');
    const data = config();
    const result = await api('events', { ...data, demo });
    if (id !== scanId || !view.isConnected) return;
    storySelection = { demo, data };
    const selectRecord = eventsResult({ ...result, rows: result.rows.slice(0, 30) }, view, details, status, data.source || '', true);
    if (result.rows.length > 30) view.append(actions(button('See all events', () => go('find', demo))));
    next(result.rows.length ? (demo ? 'Read the next check, then select Show the evidence to see the Windows record.'
      : 'Select an item in the timeline to see what it means. Use Save report to keep a copy.')
      : 'Parsley found no matching records. Try a wider time range.');
    if (result.case) {
      header.querySelector('h1').textContent = result.case.title;
      const evidence = result.rows.find(event => Number(event.id) === result.case.evidence_event_id);
      caseNotes.replaceChildren(title('What Windows recorded'), p(result.case.summary),
        el('div', 'banner', el('strong', '', 'Next check'), p(result.case.next),
          evidence ? actions(button('Show the evidence', () => {
            selectRecord(evidence);
            const heading = details.querySelector('h2');
            heading.tabIndex = -1;
            heading.focus();
          }, 'green'), helpLink(evidence.help, status)) : undefined));
    }
  };
  function visibility() {
    input.hidden = range.value() === 'day';
    at.view.firstChild.textContent = range.value() === 'file' ? 'When the problem happened (optional)' : 'When the problem happened';
    source.view.hidden = range.value() !== 'file';
  }
  visibility();
  range.control.addEventListener('change', visibility);
  picker.control.addEventListener('change', () => scan().catch(error => tell(status, error.message, 'error')));
  if (demo) {
    api('demo-cases').then(cases => {
      picker.control.replaceChildren(...cases.map(item => {
        const option = el('option', '', item.title);
        option.value = item.id;
        return option;
      }));
      picker.control.value = caseId || 'restart';
      return scan();
    }).catch(error => tell(status, error.message, 'error'));
  } else if (scanLatest) {
    scan().catch(error => tell(status, error.message, 'error'));
  }
  return el('div', '', header,
    card(demo ? picker.view : stack(range.view, input, source.view),
      actions(demo ? null : work('Check Windows records', status, scan, 'green'),
        work('Save report', status, () => saveReport('timeline', config(), 'html', status, 'Rewind'), demo ? 'green' : '')), status),
    el('div', 'split wide-left', view, demo ? stack(card(caseNotes), details) : details),
    el('details', 'card advanced', el('summary', '', 'More options'),
      demo ? null : actions(button('Compare with a normal day', () => go('compare'))),
      casePanel(demo, () => {
        const data = config();
        if (!demo && !data.at) throw new Error('Choose the incident time to save a case.');
        return data;
      })));
}
function setCase(path) {
  caseFile = path;
  for (const view of document.querySelectorAll('.case-path')) view.textContent = path;
}
function casePanel(demo, settings) {
  const status = msg();
  const shown = el('p', 'case-path', caseFile || 'No incident case open.');
  const debuggerFile = file('WinDbg text report', 'debugger', status);
  const debuggerAt = field('Crash time in the debugger report', 'datetime-local');
  const module = check('Include the faulting module name');
  const create = work(demo ? 'Save fictional case…' : 'Save incident case…', status, async () => {
    const data = demo ? {} : { ...settings(), debugger: debuggerFile.value(), debuggerAt: date(debuggerAt), includeModule: module.value() };
    if (data.debugger && !data.debuggerAt) throw new Error('Choose the crash time from the debugger report.');
    const path = await api('pick', { kind: 'json' });
    if (!path) return cancelled(status);
    tell(status, 'Saving the incident case…', 'busy');
    try { tell(status, await api(demo ? 'case-demo' : 'case-create', { ...data, path })); }
    catch (error) { throw new Error('Could not save the case. ' + error.message); }
    setCase(path);
    next('Add records from the timeline to the case, then save the redacted case report.');
  });
  const open = work('Open case…', status, async () => {
    const path = await api('pick', { kind: 'case' });
    if (!path) return;
    setCase(path);
    tell(status, 'Opened the case. Records you open on the timeline can now be added to it.');
  });
  const report = work('Save case report…', status, async () => {
    if (!caseFile) throw new Error('Save or open an incident case first.');
    const path = await api('pick', { kind: 'html' });
    if (!path) return cancelled(status);
    tell(status, 'Saving the redacted case report…', 'busy');
    try { tell(status, await api('case-report', { input: caseFile, path })); }
    catch (error) { throw new Error('Could not save the case report. ' + error.message); }
    showSavedReport(status, path, 'Incident case report');
  });
  return card(el('div', 'row between', title('Incident case'), el('span', 'badge quiet', 'Saved on this PC')),
    p('Keep the records, bookmarks and debugger evidence for one incident in a local case file. Each change saves a new copy, and the case report is redacted.'),
    demo ? null : el('details', '', el('summary', '', 'Add debugger evidence (optional)'),
      el('div', 'fieldrow', debuggerFile.view, debuggerAt.view), module.view),
    actions(create, open, report), shown, status);
}

function compare() {
  const demo = fictional;
  const status = msg();
  const incident = field('When the problem happened', 'datetime-local');
  const baseline = field('A time when the PC was working normally', 'datetime-local');
  const radius = field('Minutes on each side', 'number', '10');
  const logs = file('Saved Windows log (.evtx)', 'event', status);
  const health = file('Health recording from the problem', 'health', status);
  const baseHealth = file('Health recording from a normal period', 'health', status);
  const result = el('div', 'grid');
  const config = () => {
    if (demo) return {};
    if (!date(incident) || !date(baseline)) throw new Error('Choose when the problem happened and a time when the PC was working normally.');
    return { at: date(incident), baseline: date(baseline), minutes: radius.value(), source: logs.value(), health: health.value(), baseHealth: baseHealth.value() };
  };
  return el('div', '', heading('Compare', 'What changed when the problem happened?', 'See what Windows recorded during the problem alongside a period when the PC worked normally.', demo ? 'Select Compare sessions to see the example restart next to the day before.' : 'Choose both times, then select Compare sessions.'),
    card(demo ? p('Compare a fictional restart with the previous day.') :
      stack(el('div', 'fieldrow', incident.view, baseline.view, radius.view),
        el('details', 'attach', el('summary', '', 'Use saved records or readings'),
          el('div', 'fieldrow', logs.view, health.view, baseHealth.view))),
      actions(work('Compare sessions', status, async () => {
        tell(status, 'Comparing the two windows…', 'busy');
        preview(result, await api('report', { ...config(), kind: 'compare', demo }));
        tell(status, 'Comparison ready.');
        next('Look for differences between the two times. Save the comparison if you want to keep it.');
      }, 'green'), work('Save comparison', status, () => saveReport('compare', config(), 'html', status, 'Session comparison'))), status), result);
}

function network() {
  const demo = fictional;
  const socketsStatus = msg();
  const status = msg();
  const rows = el('div', '');
  const endpointRows = el('pre');
  const rate = number => number < 1000 ? Math.round(number) + ' B/s' : (number / 1000).toFixed(1) + ' kB/s';
  const refresh = async () => {
    tell(socketsStatus, 'Reading endpoint counts…', 'busy');
    endpointRows.textContent = await api('endpoints', { demo });
    tell(socketsStatus, '');
  };
  const capture = async () => {
    rows.replaceChildren();
    save.hidden = true;
    tell(status, demo ? 'Loading sample data…' : 'Approve the Windows prompt to begin the 15-second capture.');
    try {
      const result = await api('bandwidth', { demo });
      const seconds = result.millis / 1000;
      const summary = seconds.toFixed(1) + ' seconds sampled.';
      const partial = result.events_lost || result.buffers_lost || result.undecoded;
      const overflow = result.overflow.sent + result.overflow.received;
      const lost = ' ' + num(result.events_lost) + ' lost events, ' + num(result.buffers_lost) + ' lost buffers and ' + num(result.undecoded) + ' undecoded events.';
      tell(status, (partial ? 'Partial capture. ' + summary + lost : summary) +
        (result.rows.length || overflow ? '' : ' No activity observed.'), partial ? 'warn' : '');
      if (result.rows.length || overflow) {
        const largest = Math.max(1, ...result.rows.map(item => item.bytes.received + item.bytes.sent), overflow);
        const values = result.rows.map(item => [
          el('div', 'network-app', item.name || el('em', '', 'Name not confirmed'),
            trafficBar((item.bytes.received + item.bytes.sent) / largest)),
          num(item.pid || null), num(item.bytes.received), num(item.bytes.sent), rate((item.bytes.received + item.bytes.sent) / seconds),
        ]);
        if (overflow) values.push(['Other process IDs', '—', num(result.overflow.received), num(result.overflow.sent), rate(overflow / seconds)]);
        rows.replaceChildren(table(['Process', 'PID', 'Received (B)', 'Sent (B)', 'Avg total rate'], values));
      }
      save.hidden = false;
    } catch (error) {
      const refused = /cancelled|canceled/i.test(error.message);
      tell(status, (refused ? 'Permission cancelled. Nothing was captured. ' : 'Bandwidth capture unavailable. ') + error.message, refused ? 'warn' : 'error');
    }
  };
  const save = work('Save capture report', status, async () => {
    const path = await api('pick', { kind: 'html' });
    if (!path) return cancelled(status);
    try { await api('bandwidth-save', { path }); }
    catch (error) { throw new Error('Could not save the report. ' + error.message); }
    showSavedReport(status, path, 'Bandwidth capture');
  });
  save.hidden = true;
  if (!window.parsleySmoke) refresh().catch(error => tell(socketsStatus, error.message, 'error'));
  const trafficBar = fraction => {
    const meter = el('div', 'traffic-bar', el('i'));
    meter.setAttribute('aria-hidden', 'true');
    meter.firstChild.style.width = (fraction * 100) + '%';
    return meter;
  };
  return el('div', '', heading('Live', 'What is using the network?', 'Measure how much data each app or Windows process sends and receives.', 'Measure for 15 seconds while the slowdown is happening.'),
    connectionObservation(demo),
    el('div', 'split',
    card(title('Measure network use'), p('See which apps and processes send or receive the most data during the sample.'),
      demo ? null : p('Windows asks for permission before measuring.'),
      actions(work(demo ? 'Show example' : 'Measure for 15 seconds', status, capture, 'green'), save), status, rows),
    card(title('Connections by app'),
      p('Counts of open connections and listening endpoints by process.'),
      actions(work('Refresh connections', socketsStatus, refresh)), socketsStatus, endpointRows)));
}

function connectionObservation(demo) {
  const status = msg();
  const results = el('div');
  const onlyDiagnostics = check('Show possible diagnostics and error reporting only');
  onlyDiagnostics.view.hidden = true;
  let observation;
  const render = () => {
    const rows = observation.rows.filter(peer => !onlyDiagnostics.value() || /diagnostics|error reporting/i.test(peer.purpose || ''));
    if (!rows.length) {
      results.replaceChildren(emptyState(onlyDiagnostics.value()
        ? 'No observed destinations matched those labels.' : 'No active connections to another device were seen.'));
      return;
    }
    const byApp = new Map();
    for (const peer of rows) {
      if (!byApp.has(peer.app)) byApp.set(peer.app, []);
      byApp.get(peer.app).push(peer);
    }
    const groups = [...byApp].sort(([a, aRows], [b, bRows]) =>
      Number(!a) - Number(!b) || bRows.length - aRows.length ||
      (a || '').localeCompare(b || ''));
    results.replaceChildren(p(num(rows.length) + ' app-and-destination rows · ' + num(groups.length) + ' groups'),
      el('div', 'stack destination-groups', groups.map(([name, peers]) => {
        const destinations = new Set(peers.map(peer => peer.address)).size;
        const services = [...new Set(peers.flatMap(peer => peer.services))].sort();
        const hinted = peers.filter(peer => peer.purpose).length;
        const details = el('details', 'destination-group',
          el('summary', '',
            el('span', '', name || 'Unidentified processes'),
            el('span', 'badge quiet', destinations + (destinations === 1 ? ' destination' : ' destinations')),
            hinted ? el('span', 'badge', hinted + (hinted === 1 ? ' possible match' : ' possible matches')) : null),
          services.length ? p('Windows lists these services under the process: ' + services.join(', ') + '.') : null,
          table(['Destination', 'Possible purpose', 'Seen', 'Process ID'], peers.map(peer => [
            stack(
              peer.possible_name ? el('strong', '', peer.possible_name) : null,
              peer.domain_source === 'app-dns' ? el('small', 'process-source', 'Possible match from this app’s DNS') : null,
              peer.domain_source === 'windows-cache' ? el('small', 'process-source', 'Possible match from the Windows DNS cache') : null,
              el('span', 'numeric', peer.address),
              peer.operator ? el('small', 'process-source', 'Network operator · ' + peer.operator) : null),
            peer.purpose ? el('span', 'badge', peer.purpose) : 'Purpose unknown',
            peer.seen + ' of ' + observation.samples,
            el('span', 'numeric', String(peer.pid),
              peer.services.length ? el('small', 'process-source', peer.services.join(' · ')) : null,
              peer.name_source === 'windows-list' ? el('small', 'process-source', 'Name from Windows process list') : null),
          ])));
        details.open = onlyDiagnostics.value();
        return details;
      })));
  };
  const observe = work(demo ? 'Show example destinations' : 'Watch connections for 8 seconds', status, async () => {
    results.replaceChildren();
    onlyDiagnostics.view.hidden = true;
    onlyDiagnostics.control.checked = false;
    tell(status, 'Reading active Windows connections…', 'busy');
    observation = await api('observe-connections', { demo });
    render();
    onlyDiagnostics.view.hidden = !observation.rows.length;
    const notes = [observation.samples + ' readings over ' + observation.seconds + ' seconds.'];
    if (observation.incomplete) notes.push('Windows missed part of the connection data.');
    if (observation.rows.length && !observation.named_destinations) notes.push('Windows didn’t provide a clear domain match.');
    if (observation.omitted) notes.push('Showing up to 150 destinations.');
    tell(status, notes.join(' '), observation.incomplete || observation.omitted ? 'warn' : '');
  }, 'green');
  onlyDiagnostics.control.addEventListener('change', () => { if (observation) render(); });
  return card(title('Who’s my PC talking to?'),
    p('See which apps have active TCP connections and which destinations show up repeatedly. Results stay in this view.'),
    actions(observe), status, onlyDiagnostics.view, results,
    el('details', 'record-details', el('summary', '', 'How to read these results'),
      p('Parsley can suggest a domain from this app’s recent DNS records when Windows already has them, or from Windows’ shared DNS cache. Known domains may have a possible purpose.'),
      p('A network operator runs the IP address range and may host many different sites. These readings show connections. What the app sent and whether it was tracking remain unknown.'),
      p('Some protected Windows processes only provide a name from the process list. Windows can also list several services under one process ID. Those names give context for the group.'),
      p('“Seen in readings” counts how many snapshots showed a connection. One open connection can appear in several readings.'),
      p('This view samples TCP connections. It can miss brief connections and apps that use UDP or their own DNS.'),
      helpLink({ id: 'windows-destinations', label: 'Microsoft’s Windows endpoint guide' }, status)));
}

function health() {
  const status = msg();
  const recordStatus = msg();
  const readings = el('div', 'metrics');
  const liveText = el('pre');
  const warn = check('Tell me if CPU or memory use stays high');
  const minutes = select('Recording duration', [
    ['5', '5 minutes'], ['15', '15 minutes'], ['30', '30 minutes'], ['60', '60 minutes'], ['custom', 'Choose a duration'],
  ]);
  minutes.control.value = '15';
  const customMinutes = field('Minutes (1–60)', 'number', '15');
  customMinutes.control.min = '1'; customMinutes.control.max = '60';
  customMinutes.view.hidden = true;
  minutes.control.addEventListener('change', () => { customMinutes.view.hidden = minutes.value() !== 'custom'; });
  const chosen = el('p', '', recordingPath || 'No recording selected.');
  const display = el('span', 'badge', 'Current snapshot');
  const snapshotStatus = msg();
  const memoryStatus = msg();
  const memoryContent = el('div');
  let memoryView = '';
  let monitoringMemory = false;
  let snapshot = '';
  let hasLiveReadings = false;
  const ram = bytes => bytes >= 1073741824 ? (bytes / 1073741824).toFixed(1) + ' GiB'
    : Math.round(bytes / 1048576) + ' MiB';
  const renderMemory = (data, monitored, running) => {
    const revision = JSON.stringify([data, monitored, running]);
    if (revision === memoryView) return;
    memoryView = revision;
    tell(memoryStatus, '');
    const available = Math.max(0, data.total_gib - data.used_gib);
    const hasAverage = data.apps.some(app => app.average_bytes != null);
    const columns = ['App or process', monitored && !running ? 'Last reading' : 'RAM now'];
    if (hasAverage) columns.push('Average · ' + data.seconds + ' s');
    memoryContent.replaceChildren(...[
      p(available.toFixed(1) + ' GiB available out of ' + data.total_gib.toFixed(1) + ' GiB.'),
      data.apps.length ? table(columns, data.apps.map(app => [
        app.name, ram(app.bytes), ...(hasAverage ? [app.average_bytes == null ? '—' : ram(app.average_bytes)] : []),
      ])) : p('Windows did not return any app memory values.'),
      data.high ? el('div', 'event-next', el('strong', '', 'Memory stayed high'),
        p('If the PC feels slow, save your work and close a large app you recognise and are done using.')) : null,
      p(hasAverage ? 'Rows are ranked by average private RAM over the recent monitoring window.'
        : 'Start monitoring to see which apps stay high. Apps with the same process name are grouped.')].filter(Boolean));
  };
  const samples = [
    ['CPU', 'cpu_percent', '%'], ['Memory', 'ram_percent', '%'], ['GPU peak', 'gpu_peak_percent', '%'],
    ['Disk', 'disk_mib_per_sec', ' MiB/s'], ['Network', 'network_kib_per_sec', ' KiB/s'],
  ];
  const redraw = data => {
    monitoringMemory ||= data.running;
    if (monitoringMemory && data.memory) renderMemory(data.memory, true, data.running);
    const firstReadings = !hasLiveReadings;
    hasLiveReadings = data.chart.length > 0;
    display.textContent = data.running ? 'Monitoring' : hasLiveReadings ? 'Monitoring stopped' : 'Current snapshot';
    display.classList.toggle('updating', data.running);
    liveText.textContent = data.latest.length ? data.latest.join('\n') : snapshot;
    startButton.disabled = data.running;
    stopButton.disabled = !data.running;
    saveButton.disabled = data.chart.length === 0;
    if (data.error) tell(status, data.error, 'error');
    if (!hasLiveReadings) {
      readings.replaceChildren();
      return;
    }
    const last = data.chart.at(-1);
    readings.replaceChildren(...samples.map(([name, key, unit]) => {
      const latest = last?.[key];
      const values = data.chart.map(point => point[key]);
      const peak = Math.max(1, ...values.filter(value => typeof value === 'number'));
      const bars = el('div', 'bars', values.map(value => {
        const bar = el('i');
        bar.style.height = (typeof value === 'number' ? Math.max(4, value / peak * 100) : 0) + '%';
        return bar;
      }));
      bars.setAttribute('aria-hidden', 'true');
      const value = el('strong', '', latest == null ? 'Unavailable' : latest.toFixed(1) + unit);
      if (unit !== '%' || latest == null) return el('div', 'card metric', name, value, bars);
      const gauge = el('div', 'gauge' + (firstReadings ? ' gauge-in' : ''), value);
      gauge.style.setProperty('--level', Math.min(100, Math.max(0, latest)) + '%');
      return el('div', 'card metric', name, gauge);
    }));
  };
  const poll = () => api('health-poll').then(redraw).catch(error => tell(status, error.message, 'error'));
  const startButton = work('Start monitoring', status, async () => {
    await api('health-start', { warn: warn.value() });
    tell(status, '');
    setTimeout(poll, 100);
  }, 'green');
  const stopButton = work('Stop monitoring', status, async () => {
    await api('health-stop');
    setTimeout(poll, 100);
  });
  const saveButton = work('Save these readings', status, async () => {
    const path = await api('pick', { kind: 'json' });
    if (!path) return cancelled(status);
    try { tell(status, await api('health-save', { path })); }
    catch (error) { throw new Error('Could not save the readings. ' + error.message); }
  });
  stopButton.disabled = true;
  saveButton.disabled = true;
  const recentList = el('div', 'stack earlier-recordings');
  const updateRecent = () => {
    recentList.replaceChildren(el('h3', '', 'Earlier recordings'),
      earlierRecordings.length ? el('ul', 'records', earlierRecordings.map(path =>
        el('li', '', work(path.split(/[\\/]/).pop(), recordStatus, () => recording('record-status', path), 'link'))))
        : p('Recordings you save or browse to appear here.'));
  };
  const recording = async (action, path) => {
    const data = await api(action, { path, minutes: minutes.value() === 'custom' ? customMinutes.value() : minutes.value() });
    recordingPath = data.path;
    chosen.textContent = recordingPath;
    checkRecording.disabled = stopRecording.disabled = !recordingPath;
    tell(recordStatus, data.status);
    if (recordingPath) {
      earlierRecordings = [recordingPath, ...earlierRecordings.filter(value => value.toLowerCase() !== recordingPath.toLowerCase())].slice(0, 8);
      try { localStorage.setItem('parsley-recordings', JSON.stringify(earlierRecordings)); } catch {}
      updateRecent();
    }
  };
  const checkRecording = work('Check recording', recordStatus, () => recording('record-status', recordingPath));
  const stopRecording = work('Stop recording', recordStatus, () => recording('record-stop', recordingPath));
  checkRecording.disabled = stopRecording.disabled = !recordingPath;
  updateRecent();
  const recorder = card(title('Keep watch while you’re away'), p('Record while signed in, even after closing this window.'),
    el('div', 'fieldrow', minutes.view, customMinutes.view), chosen, actions(work('Start recording', recordStatus, async () => {
      const path = await api('pick', { kind: 'jsonl' });
      if (path) await recording('record-start', path);
    }, 'green'), work('Browse earlier recordings', recordStatus, async () => {
      const path = await api('pick', { kind: 'journal' });
      if (path) await recording('record-status', path);
    }), checkRecording, stopRecording), recordStatus, recentList);
  if (window.parsleySmoke) {
    const chart = Array.from({ length: 60 }, () =>
      Object.fromEntries(samples.map(([, key]) => [key, 50])));
    redraw({ running: false, chart, latest: [] });
    renderMemory({ used_gib: 29, total_gib: 32, seconds: 30, high: true, apps: [
      { name: 'example-app', bytes: 1073741824, average_bytes: 2147483648 },
    ] }, true, true);
  } else {
    tell(memoryStatus, 'Reading app memory…', 'busy');
    api('memory-now').then(data => {
      if (!monitoringMemory || !memoryView) renderMemory(data, false, false);
    }).catch(error => {
      if (!memoryView) tell(memoryStatus, error.message, 'warn');
    });
    tell(snapshotStatus, 'Reading this PC…', 'busy');
    api('system', { demo: false }).then(data => {
      snapshot = data;
      if (!hasLiveReadings) liveText.textContent = snapshot;
      tell(snapshotStatus, '');
    }).catch(error => {
      if (!hasLiveReadings) tell(snapshotStatus, 'Could not read this PC. ' + error.message, 'error');
    });
    poll();
    interval(poll, 1000);
  }
  return el('div', '', heading('Live', 'Right now', 'See processor, memory, storage and network readings from this PC.',
    'Start monitoring to watch the readings change while you use your PC.'),
    card(el('div', 'row between', title('Readings from this PC'), display), snapshotStatus, readings, warn.view,
      actions(startButton, stopButton, saveButton), status,
      el('details', 'record-details', el('summary', '', 'More system readings'), liveText)),
    el('div', 'split', card(title('Memory by app'), memoryStatus, memoryContent), recorder));
}

function savedItems(items, query, status, showRecord, load) {
  const list = el('div', 'stack');
  if (fictional) return list;
  const render = data => {
    list.replaceChildren(title('Saved searches and bookmarks'));
    if (!data.searches.length && !data.bookmarks.length) list.append(p('Saved searches and bookmarks appear here.'));
    for (const search of data.searches) {
      list.append(row(button(search, () => { query.control.value = search; load().catch(error => tell(status, error.message, 'error')); }, 'link'),
        work('Remove search', status, async () => render(await api('search-remove', { query: search })))));
    }
    for (const bookmark of data.bookmarks) {
      list.append(row(button(bookmark.title + ' · ' + bookmark.record, () => showRecord(bookmark), 'link'),
        work('Remove bookmark', status, async () => render(await api('bookmark-remove', { bookmark })))));
    }
  };
  items.render = render;
  return list;
}
function frequencyBars(counts) {
  const peak = Math.max(1, ...counts);
  const bars = el('div', 'bars freq', counts.map(count => {
    const bar = el('i');
    bar.style.height = (count ? Math.max(12, count / peak * 100) : 3) + '%';
    if (!count) bar.className = 'zero';
    return bar;
  }));
  bars.setAttribute('role', 'img');
  bars.setAttribute('aria-label', counts.reduce((a, b) => a + b, 0) + ' incidents over ' + counts.length + ' days');
  return bars;
}
const basisLabel = { signature: 'Same signature', 'stop-code': 'Same stop code', unexplained: 'Unexplained', 'evidence-poor': 'Missing evidence' };
function groupCard(group, days, details, status) {
  const counts = group.frequency['days' + days] || group.frequency.days30;
  const records = el('ul', 'records', group.records.map(record => el('li', '',
    button(displayTime(record.at) + ' · ' + record.title + ' · Record ' + record.record, () => eventDetails(details, record, status), 'link'),
    record.linked ? el('span', 'badge', 'Linked pair') : null)));
  const poor = group.basis === 'unexplained' || group.basis === 'evidence-poor';
  return el('article', 'card group' + (poor ? ' poor' : ''),
    el('div', 'row between', el('h3', '', group.title), el('span', 'badge' + (poor ? ' warn' : ''), basisLabel[group.basis] || group.basis)),
    el('div', 'row facts', el('strong', 'numeric', num(group.incidents) + (group.incidents === 1 ? ' incident' : ' incidents')),
      el('span', '', 'First ' + displayTime(group.first)), el('span', '', 'Last ' + displayTime(group.last))),
    frequencyBars(counts), p(group.explanation),
    el('details', '', el('summary', '', 'Evidence behind this group'), el('ul', '', group.evidence.map(line => el('li', '', line)))),
    group.clues.length ? el('details', '', el('summary', '', 'Around the incident'), el('ul', '', group.clues.map(line => el('li', '', line)))) : null,
    nextStep(group.next_check, group.help, status),
    el('details', '', el('summary', '', 'Windows records (' + group.records.length + ')'), records));
}
function crashes() {
  const demo = fictional;
  const status = msg();
  const days = select('History window', [['30', '30 days'], ['7', '7 days'], ['90', '90 days']]);
  const query = field('Filter by word or Windows record number');
  const names = check('Show app and component names');
  names.control.checked = demo;
  names.control.disabled = demo;
  const healthFile = file('Health recording', 'health', status);
  const before = file('Inventory snapshot before', 'version', status);
  const after = file('Inventory snapshot after', 'version', status);
  const groups = el('div', 'stack');
  const list = card(title('Windows records'));
  const details = card(title('Event details'), p('Open a record from a group to see its explanation.'));
  const totals = el('div', 'row');
  const saved = {};
  let feed;
  let polling = false;
  let revision = 0;
  const options = () => demo ? { days: days.value() } : {
    days: days.value(), names: names.value(), health: healthFile.value(), before: before.value(), after: after.value(),
  };
  const load = async (live = false) => {
    const version = ++revision;
    if (!live) tell(status, 'Reading crash history…', 'busy');
    const data = await api(live ? 'events' : 'history', { ...options(), demo, query: query.value(), live });
    if (version !== revision || !groups.isConnected) return;
    eventsResult(data, list, details, status, '');
    totals.replaceChildren(...Object.entries(data.counts).map(([name, total]) => el('span', 'badge', name + ' · ' + total)));
    if (data.analysis) {
      const analysis = data.analysis;
      groups.replaceChildren(...[
        el('div', 'row between', title('Recurring incidents'),
          el('span', 'badge', num(analysis.incidents) + ' incidents · ' + analysis.groups.length + ' groups')),
        ...analysis.notices.map(text => el('div', 'banner warn', text)),
        analysis.groups.length ? null : emptyState('No crashes, stop errors or unexpected restarts in this window.'),
        ...analysis.groups.map(group => groupCard(group, days.value(), details, status))].filter(Boolean));
      if (!live) next(analysis.groups.length ? 'Open the first group, check its evidence and follow its next check.' : 'Nothing recurring here. Try the 90-day window or start live events.');
      groups.dataset.incidents = analysis.incidents;
    }
  };
  const showRecord = marker => eventDetails(details, {
    title: marker.title, at: marker.time_utc, channel: marker.channel, provider: 'Windows Event Log',
    id: '—', record: String(marker.record), explanation: 'Open the original record to check that it is still available.',
  }, status);
  const local = savedItems(saved, query, status, showRecord, load);
  const toggle = work('Start live events', status, async () => {
    if (polling) {
      polling = false; ++revision; clearInterval(feed);
      toggle.textContent = 'Start live events'; tell(status, 'Live events stopped.');
      return;
    }
    polling = true; toggle.textContent = 'Stop live events';
    const poll = () => load(true).catch(error => tell(status, error.message, 'error'));
    await poll();
    if (!polling) return;
    feed = setInterval(poll, 10000);
  });
  cleanups.push(() => { polling = false; ++revision; clearInterval(feed); });
  const history = () => { polling = false; clearInterval(feed); toggle.textContent = 'Start live events'; return load(); };
  if (!demo && !window.parsleySmoke) api('saved').then(saved.render).catch(error => tell(status, error.message, 'error'));
  if (demo) history().catch(error => tell(status, error.message, 'error'));
  return el('div', '', heading('What happened', 'Which problems keep coming back?', 'See repeat crashes and restarts together. Each group explains why the records belong together.', 'Load crash history, then open the group that matches what you noticed.'),
    card(el('div', 'fieldrow', days.view, query.view), names.view,
      demo ? null : el('details', 'attach', el('summary', '', 'Attach local evidence (optional)'),
        p('A health recording or two inventory snapshots can show what changed around an incident. Timing is a clue, not a cause.'),
        el('div', 'fieldrow', healthFile.view, before.view, after.view)),
      actions(work('Load crash history', status, history, 'green'), demo ? null : toggle,
        demo ? null : work('Save search', status, async () => saved.render(await api('search-save', { query: query.value() }))),
        work('Save history report', status, () => saveReport('history', options(), 'html', status, 'Crash history'))),
      status, totals),
    groups, el('div', 'split', list, details), demo ? null : card(local));
}

function inventory() {
  const demo = fictional;
  const status = msg();
  const data = el('pre');
  const compareStatus = msg();
  const before = file('Older inventory file', 'version', compareStatus);
  const after = file('Newer inventory file', 'version', compareStatus);
  const result = el('div', 'grid');
  const config = () => {
    if (demo) return {};
    if (!before.value() || !after.value()) throw new Error('Choose both version snapshots.');
    return { before: before.value(), after: after.value() };
  };
  return el('div', '', heading('My PC', 'What is installed on this PC?', 'See your computer’s hardware, software and storage.', demo ? 'View the example inventory, then compare versions.' : 'View what is installed now. Save an inventory file if you want to check for changes later.'),
    el('div', 'split',
    card(title('Device and software inventory'),
      p('Lists hardware, services and desktop apps registered with Windows. Some Store apps and portable apps may be missing.'),
      actions(work('View inventory', status, async () => {
      tell(status, 'Reading inventory…', 'busy');
      data.textContent = await api('inventory', { demo });
      tell(status, '');
    }, 'green'), work('Save readable report', status, () => saveReport('inventory', {}, 'html', status, 'Device inventory'))),
      status, data),
    card(title('Check what changed'),
      demo ? p('See a fictional before and after.') : stack(
        p('Save a comparison file now and another after a change.'),
        actions(work('Save comparison file (.json)', compareStatus, () => saveReport('inventory-save', {}, 'json', compareStatus, 'Inventory snapshot'))),
        el('div', 'fieldrow', before.view, after.view)),
      actions(work('Compare inventories', compareStatus, async () => {
        preview(result, await api('report', { kind: 'inventory-compare', ...config(), demo }));
        tell(compareStatus, 'Comparison ready.');
      }, 'green'), work('Save comparison', compareStatus, () => saveReport('inventory-compare', config(), 'html', compareStatus, 'Inventory version comparison'))), compareStatus)),
    result);
}
function exports() {
  const demo = fictional;
  const timelineStatus = msg();
  const format = select('File type', [['html', 'Web page (HTML)'], ['csv', 'Spreadsheet (CSV)'], ['json', 'Structured data (JSON)']]);
  const names = check('Include app and component names in the web page');
  const result = el('div', 'grid');
  format.control.addEventListener('change', () => { names.control.disabled = format.value() !== 'html'; });
  const timelineData = () => ({ names: format.value() === 'html' && names.value() });
  return el('div', '', heading('How to and help', 'Export Windows records', 'Choose a format and review what your report includes.'),
    card(title('Event timeline'), p('Save the Windows records from the last 24 hours.'), format.view, names.view,
      actions(work('Preview HTML', timelineStatus, async () => {
        tell(timelineStatus, 'Building a local preview…', 'busy');
        preview(result, await api('report', { kind: 'timeline', ...timelineData(), demo }));
        tell(timelineStatus, 'Preview ready.');
      }), work('Save event export', timelineStatus, () => saveReport('timeline', timelineData(), format.value(), timelineStatus, 'Event timeline'), 'green')),
      timelineStatus), result);
}

function supportReports() {
  const demo = fictional;
  const status = msg();
  const result = el('div', 'grid');
  const health = check('Include current system readings');
  const inventory = check('Include installed hardware and software');
  const options = () => ({ health: health.value(), inventory: inventory.value() });
  return el('div', '', card(title('Make a support report'),
    p('Start with a timeline that hides app and component names. Add device details if the person helping you needs them.'),
    health.view, inventory.view,
    actions(work('Preview support report', status, async () => {
      tell(status, 'Building a local preview…', 'busy');
      preview(result, await api('report', { kind: 'support', ...options(), demo }));
      tell(status, 'Preview ready.');
    }), work('Save support report', status, () => saveReport('support', options(), 'html', status, 'Support report'), 'green')), status), result);
}
function find() {
  const demo = fictional;
  const status = msg();
  const query = field('Search recent Windows records');
  const source = file('Windows log file (.evtx)', 'event', status);
  query.control.placeholder = 'For example, restart, display, or record number';
  const filters = el('div', 'row filters');
  filters.setAttribute('aria-label', 'Filter by event type');
  filters.setAttribute('role', 'group');
  const view = card(title('Windows records'), emptyState('Search for a word like restart or display to get started.'));
  const details = card(title('Event details'), p('Select a result to see what Windows recorded.'));
  const saved = {};
  let request = 0;
  let timer;
  const search = async () => {
    const revision = ++request;
    const scope = source.value() ? { source: source.value() } : storySelection?.demo === demo ? storySelection.data : {};
    tell(status, 'Searching Windows records…', 'busy');
    const result = await api('events', { ...scope, demo, query: query.value() });
    if (!view.isConnected || revision !== request) return;
    const show = key => {
      const rows = key ? result.rows.filter(event => event.kind === key) : result.rows;
      const bookmark = !demo && !scope.source ? event => {
        const record = Number(event.record);
        if (!Number.isSafeInteger(record)) return '—';
        const star = work('Bookmark', status, async () => {
          saved.render(await api('bookmark-save', { bookmark: { channel: event.channel, record, title: event.title, time_utc: event.at } }));
          tell(status, 'Windows record bookmarked.');
        }, 'link');
        star.setAttribute('aria-label', 'Bookmark ' + event.title + ', record ' + event.record);
        return star;
      } : null;
      eventsResult(key ? { ...result, rows, count: rows.length } : result, view, details, status, scope.source || '', false, bookmark);
      for (const control of filters.children) control.setAttribute('aria-pressed', String(control.dataset.kind === key));
    };
    const kinds = [['', 'All'], ['restart', 'Restarts'], ['app', 'Apps'], ['graphics', 'Display'],
      ['hardware', 'Hardware'], ['service', 'Services'], ['update', 'Updates'], ['power', 'Sleep & wake']];
    filters.replaceChildren(...kinds.flatMap(([key, label]) => {
      const count = key ? result.rows.filter(event => event.kind === key).length : result.rows.length;
      if (key && !count) return [];
      const control = button(label + ' · ' + num(count), () => show(key), 'filter');
      control.dataset.kind = key;
      return [control];
    }));
    show('');
    next(result.rows.length ? 'Select a record to see what it means.' : 'Try a broader word or choose another time in Story.');
  };
  const showBookmark = bookmark => eventDetails(details, {
    title: bookmark.title, at: bookmark.time_utc, channel: bookmark.channel, provider: 'Windows Event Log',
    id: '—', record: String(bookmark.record), explanation: 'Open the original record to check that it is still available.',
  }, status);
  const local = savedItems(saved, query, status, showBookmark, search);
  const load = () => search().catch(error => tell(status, error.message, 'error'));
  if (!demo && !window.parsleySmoke) api('saved').then(saved.render).catch(error => tell(status, error.message, 'error'));
  if (!window.parsleySmoke) load();
  const submit = work('Search', status, async () => { clearTimeout(timer); await search(); }, 'green');
  query.control.addEventListener('input', () => {
    clearTimeout(timer);
    ++request;
    timer = setTimeout(load, 350);
  });
  cleanups.push(() => clearTimeout(timer));
  query.control.addEventListener('keydown', event => {
    if (event.key === 'Enter') { event.preventDefault(); submit.click(); }
  });
  return el('div', '', heading('What happened', 'All events', 'Search the window selected in Story, or the last 24 hours.'),
    card(query.view, actions(submit, demo ? null : work('Save search', status, async () =>
      saved.render(await api('search-save', { query: query.value() })))),
      demo ? null : el('details', 'record-details', el('summary', '', 'Open a saved Windows log'), source.view),
      status, filters),
    el('div', 'split wide-left', view, details), demo ? null : card(local));
}

go('start');

if (window.parsleySmoke) {
  (async () => {
    const seen = [];
    for (const key of Object.keys(pages)) {
      go(key, !['start', 'help', 'health'].includes(key));
      if (!screen.querySelector('h1')) throw new Error('The ' + key + ' screen did not render.');
      seen.push(key);
    }
    const records = await api('events', { demo: true });
    const history = await api('history', { demo: true, days: '30' });
    const comparison = await api('report', { kind: 'compare', demo: true });
    const endpoints = await api('endpoints', { demo: true });
    const network = await api('bandwidth', { demo: true });
    const health = await api('system', { demo: true });
    const inventory = await api('inventory', { demo: true });
    const inventoryReport = await api('report', { kind: 'inventory', demo: true });
    const difference = await api('report', { kind: 'inventory-compare', demo: true });
    const support = await api('report', { kind: 'support', demo: true, health: true, inventory: true });
    const evidence = await api('events', { demo: true, query: 'restart' });
    const grouped = await api('history', { demo: true, days: '90' });
    const groupedReport = await api('report', { kind: 'history', demo: true, days: '90' });
    const checks = {
      rewind: records.rows.length,
      history: history.rows.length,
      compare: comparison.html.includes('Session comparison'),
      endpoints: endpoints.includes('demo-browser'),
      bandwidth: network.rows.length,
      health: health.includes('CPU'),
      inventory: inventory.includes('Example') && inventoryReport.html.includes('Fictional device inventory'),
      versions: difference.html.includes('Inventory version comparison'),
      export: support.html.includes('Support report'),
      evidence: evidence.rows.some(item => item.title.toLowerCase().includes('restart')),
      groups: grouped.analysis.groups.length > 3 &&
        groupedReport.html.includes('data-incidents="' + grouped.analysis.incidents + '"') &&
        grouped.analysis.groups.every(group => groupedReport.html.includes('data-incidents="' + group.incidents + '"') &&
          group.records.every(record => groupedReport.html.includes('id="' + record.channel + '-' + record.record + '"'))),
    };
    go('start');
    checks.lead = screen.querySelector('.hero')?.textContent.includes('Check the last 24 hours') &&
      Boolean(screen.querySelector('.next')) && nav.querySelectorAll('button').length === 5;
    checks.startOnlyReal = !screen.querySelector('[aria-label="Practice cases"]') &&
      [...nav.querySelectorAll('button')].some(control => control.textContent.includes('How to'));
    [...screen.querySelectorAll('.hero button')].find(control => control.textContent === 'Choose a time')?.click();
    checks.chooseTime = screen.querySelector('select')?.value === 'at' &&
      screen.querySelector('input[type=datetime-local]')?.closest('[hidden]') === null;
    go('rewind', true);
    checks.casePanel = [...screen.querySelectorAll('button')].some(item => item.textContent === 'Save fictional case…') &&
      [...screen.querySelectorAll('button')].some(item => item.textContent === 'Save report');
    checks.next = Boolean(screen.querySelector('.next'));
    go('rewind');
    const guidanceCard = card();
    eventDetails(guidanceCard, records.rows.find(event => event.help?.id === 'restart'), msg());
    checks.realGuidance = guidanceCard.querySelector('.event-next')?.textContent.includes('Next check') &&
      guidanceCard.querySelector('.event-next button')?.textContent.includes('Microsoft help') &&
      guidanceCard.querySelector('.record-details')?.open === false;
    checks.moreOptions = screen.querySelector('details.advanced')?.open === false;
    checks.dropdown = !CSS.supports('appearance', 'base-select') ||
      getComputedStyle(screen.querySelector('select')).appearance === 'base-select';
    const failed = Object.keys(checks).filter(key => !checks[key]);
    if (failed.length) throw new Error('Screen checks failed: ' + failed.join(', '));
    const findButton = label => [...screen.querySelectorAll('button')].find(button => button.textContent === label);
    const waitFor = async (read, description) => {
      const until = Date.now() + 4000;
      while (!read() && Date.now() < until) {
        await new Promise(resolve => setTimeout(resolve, 25));
      }
      const value = read();
      if (!value) throw new Error('Could not find ' + description);
      return value;
    };
    for (const item of await api('demo-cases')) {
      go('help');
      const practice = screen.querySelector('details');
      if (!practice || practice.open) throw new Error('Practice cases must start collapsed in How to.');
      practice.open = true;
      (await waitFor(() => findButton(item.title), item.title)).click();
      const showEvidence = await waitFor(() => findButton('Show the evidence'), 'Show the evidence');
      const data = await api('events', { demo: true, case: item.id });
      const report = await api('report', { demo: true, kind: 'timeline', case: item.id });
      showEvidence.click();
      const evidence = data.rows.find(event => Number(event.id) === data.case?.evidence_event_id);
      const selected = screen.querySelector('.story-list li.selected');
      if (!evidence || selected?.dataset.record !== String(evidence.record) ||
          document.activeElement?.textContent !== evidence.title) {
        throw new Error('The next check did not open the expected record for ' + item.title);
      }
      if (!data.case || !screen.textContent.includes(data.case.next) ||
          !report.html.includes(data.case.title) || !report.html.includes(data.case.next) ||
          !data.rows.every(event => report.html.includes(event.title) &&
             report.html.includes('id="' + event.channel + '-' + event.record + '"'))) {
        throw new Error('Fictional case did not match its report: ' + item.id);
      }
      const allEvents = screen.querySelector('.tabs button:nth-child(2)');
      allEvents?.click();
      if (screen.getAttribute('aria-label') !== 'All events' || !fictional) {
        throw new Error('The All events tab did not keep practice mode.');
      }
      [...nav.querySelectorAll('button')].find(control => control.textContent === 'Live')?.click();
      if (fictional || !screen.textContent.includes('Readings from this PC') || !findButton('Start monitoring')) {
        throw new Error('Opening Live health from practice did not return to the real PC.');
      }
      const charts = [...screen.querySelectorAll('.metric :is(.bars,.gauge)')];
      if (charts.length !== 5 || screen.querySelectorAll('.metric .gauge').length !== 3 ||
          charts.some(chart => chart.scrollWidth > chart.clientWidth) ||
          !screen.querySelector('input[type="checkbox"]') || screen.textContent.includes('[object Object]')) {
        throw new Error('Live readings overflowed or the alerts control did not display.');
      }
      if (!screen.textContent.includes('Memory by app') || !screen.textContent.includes('Average · 30 s') ||
          !screen.textContent.includes('Memory stayed high') || !screen.textContent.includes('example-app')) {
        throw new Error('Live health did not show the sustained app memory reading.');
      }
    }
    go('compare', true);
    screen.querySelector('button.green').click();
    const until = Date.now() + 4000;
    while (!screen.querySelector('iframe') && Date.now() < until) {
      await new Promise(resolve => setTimeout(resolve, 25));
    }
    const frame = screen.querySelector('iframe');
    if (!frame?.hasAttribute('sandbox') || !frame.srcdoc.includes('Session comparison')) {
      throw new Error('The in-app comparison preview did not open.');
    }
    for (const [page, action, outcome] of [
      ['events', 'Load crash history', () => screen.querySelector('[data-incidents] .group')],
      ['network', 'Show example', () => screen.querySelector('tbody tr')],
      ['network', 'Show example destinations', () => screen.textContent.includes('Possible Windows diagnostics') &&
        screen.textContent.includes('Purpose unknown') && screen.textContent.includes('4 of 5') &&
        screen.textContent.includes('Network operator · Example network') &&
        screen.textContent.includes('Possible match from this app’s DNS') &&
        screen.textContent.includes('Possible match from the Windows DNS cache') &&
        screen.textContent.includes('Example Windows service') && screen.textContent.includes('Name from Windows process list') &&
        screen.querySelectorAll('.destination-group:not([open])').length === 2],
      ['inventory', 'View inventory', () => screen.querySelector('pre')?.textContent.includes('Example')],
      ['help', 'Preview support report', () => screen.querySelector('iframe')?.srcdoc.includes('Support report')],
      ['export', 'Preview HTML', () => screen.querySelector('iframe')?.srcdoc.includes('Windows event report')],
    ]) {
      go(page, true);
      (await waitFor(() => findButton(action), action)).click();
      await waitFor(outcome, page + ' results');
    }
    go('find', true);
    const query = screen.querySelector('input');
    query.value = 'restart';
    query.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter' }));
    await waitFor(() => screen.querySelector('tbody')?.textContent.toLowerCase().includes('restart'), 'restart search result');
    if (!screen.querySelector('.filters button[aria-pressed=true]')) throw new Error('The All events filters were missing.');
    window.ipc.postMessage(JSON.stringify({ id: 0, action: 'smoke', data: { receipt: seen.join(', ') } }));
  })().catch(error => window.ipc.postMessage(JSON.stringify({ id: 0, action: 'smoke-error', data: { receipt: error.message } })));
}
