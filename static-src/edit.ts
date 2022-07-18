import { CodeJar } from 'codejar';
import { get_id } from './dom';

const highlight = (editor: HTMLElement): void => {
  const code = editor.textContent ?? '';
  // Do something with code and set html.
  editor.innerHTML = code;
};

async function save(editor: HTMLDivElement): Promise<void> {
  const res = await fetch(location.pathname, {
    method: 'POST',
    body: editor.innerText,
  });

  if (res.redirected) {
    location.assign(res.url);
  }
}

async function create(editor: HTMLDivElement): Promise<void> {
  const format_select = get_id<HTMLSelectElement>('format');
  const format = format_select.options[format_select.selectedIndex].value;

  const res = await fetch(location.pathname, {
    method: 'POST',
    headers: {
      Accept: 'application/json',
      'content-type': 'application/json',
    },
    body: JSON.stringify({
      format,
      body: editor.innerText,
    }),
  });

  if (res.redirected) {
    location.assign(res.url);
  }
}

async function preview(
  editor: HTMLDivElement,
  preview: HTMLDivElement,
  jar: CodeJar,
): Promise<void> {
  jar.recordHistory();

  const format_select = get_id<HTMLSelectElement>('format');
  const format = format_select.options[format_select.selectedIndex].value;

  let req = '/meta/render';

  if (format != null) {
    req += `?format=${format}`;
  }

  const res = await fetch(req, {
    method: 'POST',
    body: editor.innerText,
  });
  const html = await res.text();

  editor.classList.add('hidden');

  preview.innerHTML = html;
}

async function edit(
  editor: HTMLDivElement,
  preview: HTMLDivElement,
  _: CodeJar,
): Promise<void> {
  editor.classList.remove('hidden');

  preview.innerHTML = '';
}

function preview_edit_toggle(
  editor_el: HTMLDivElement,
  preview_el: HTMLDivElement,
  jar: CodeJar,
): void {
  const el = get_id<HTMLInputElement>('preview-toggle');

  el.addEventListener('change', event => {
    const target: HTMLInputElement = event.target as any;
    if (target.checked) {
      preview(editor_el, preview_el, jar).catch(console.error);
    } else {
      edit(editor_el, preview_el, jar).catch(console.error);
    }
  });
}

export async function setup_editor(): Promise<void> {
  const path = location.pathname.replace('/meta/edit', '/meta/raw');
  const res = await fetch(path);
  const code = await res.text();

  const editor = get_id<HTMLDivElement>('editor');
  const jar = CodeJar(editor, highlight, { spellcheck: true });

  jar.updateCode(code);

  preview_edit_toggle(editor, get_id('preview'), jar);

  get_id('save').addEventListener('click', () => {
    save(editor).catch(() => {});
  });
}

export async function newpage_editor(): Promise<void> {
  const editor = get_id<HTMLDivElement>('editor');
  const jar = CodeJar(editor, highlight, { spellcheck: true });

  preview_edit_toggle(editor, get_id('preview'), jar);

  get_id('save').addEventListener('click', () => {
    create(editor).catch(() => {});
  });
}
