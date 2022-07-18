// @ts-expect-error
import css_has_polyfill from 'css-has-pseudo/browser';

import './styles/style.pcss';

import './color_scheme';

css_has_polyfill(document);

export { setup_editor, newpage_editor } from './edit';
