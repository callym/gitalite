const postcss_preset_env = require('postcss-preset-env');
const postcss_import = require('postcss-import');

const present_env = postcss_preset_env({
  stage: 2,
  features: {
    'nesting-rules': true,
  },
});

module.exports = {
  plugins: [postcss_import(), present_env],
};
