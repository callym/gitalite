module.exports = {
  env: {
    es2021: true,
    node: true,
  },
  extends: [
    'standard-with-typescript',
    'prettier',
    'prettier/@typescript-eslint',
    'prettier/standard',
  ],
  parser: '@typescript-eslint/parser',
  parserOptions: {
    ecmaVersion: 12,
    project: './tsconfig.json',
    sourceType: 'module',
  },
  plugins: ['@typescript-eslint'],
  rules: {
    camelcase: 'off',
    'dot-notation': 'off',
    '@typescript-eslint/dot-notation': 'off',
    '@typescript-eslint/naming-convention': 'off',
    '@typescript-eslint/no-unused-vars': [
      'error',
      { argsIgnorePattern: '^_', varsIgnorePattern: '^_' },
    ],
  },
};
