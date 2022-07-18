import { get_id } from './dom';

const storage_key = 'color-scheme';
const color_scheme: HTMLSelectElement = get_id('color-scheme');

color_scheme.addEventListener('change', event => {
  const target: HTMLSelectElement = event.target as any;
  const selected = target.options[target.selectedIndex].value;

  do_scheme(selected as any);
});

function do_scheme(scheme: 'system' | 'light' | 'dark'): void {
  localStorage.setItem(storage_key, scheme);

  const html = document.documentElement;

  if (scheme === 'system') {
    if (window.matchMedia == null) {
      return;
    }

    if (window.matchMedia('(prefers-color-scheme: dark)').matches) {
      html.setAttribute('data-theme', 'dark');
    }
  } else {
    html.setAttribute('data-theme', scheme);
  }
}

document.addEventListener('DOMContentLoaded', () => {
  const scheme = localStorage.getItem(storage_key);

  if (scheme != null) {
    do_scheme(scheme as any);
    color_scheme.value = scheme;
  } else {
    color_scheme.value = 'system';
  }
});
