export function get_id<T extends HTMLElement>(id: string): T {
  const el = document.getElementById(id);

  if (el == null) {
    throw new Error(`Expected element with id="${id}"`);
  }

  return el as any;
}

export function get_class<T extends HTMLElement>(
  id: string,
  from?: Element,
): T[] {
  const el = Array.from((from ?? document).getElementsByClassName(id));

  if (el.length === 0) {
    throw new Error(`Expected element with class="${id}"`);
  }

  return el as any;
}

export function get_first_tag<T extends HTMLElement>(
  name: string,
  from?: Element,
): T {
  const el = (from ?? document).getElementsByTagName(name).item(0);

  if (el == null) {
    throw new Error(`Expected <${name}> element"`);
  }

  return el as any;
}
