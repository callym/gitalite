import postcss from 'rollup-plugin-postcss';
import resolve from '@rollup/plugin-node-resolve';
import typescript from '@rollup/plugin-typescript';
import copy from 'rollup-plugin-copy';

const postcss_plugin = postcss({
  plugins: [],
  extract: true,
});

const copy_plugin = copy({
  targets: [
    { src: 'node_modules/katex/dist/fonts/**/*', dest: 'static/fonts' },
  ],
});

export default {
  input: 'static-src/main.ts',
  output: {
    file: 'static/bundle.js',
    format: 'esm',
  },
  plugins: [resolve(), typescript(), postcss_plugin, copy_plugin],
};
