// Tailwind + DaisyUI config for the captive portal.
// Only the two themes we ship (light `her` + dark `afterdark`) and only the
// classes used by the portal pages are emitted, keeping the gzipped CSS tiny.
// Theme palettes are copied from ../dms/assets/tailwind.config.js.

module.exports = {
  content: ['./src/**/*.html'],
  daisyui: {
    themes: [
      {
        her: {
          primary: '#b57979',
          secondary: '#d5abab',
          accent: '#fef3c7',
          neutral: '#7f3535',
          'base-100': '#651d1d',
          info: '#7dd3fc',
          success: '#a7f3d0',
          warning: '#fef08a',
          error: '#fca5a5',
        },
        afterdark: {
          primary: '#7B79B5',
          secondary: '#ACABD5',
          accent: '#fef3c7',
          neutral: '#38357F',
          'base-100': '#201D65',
          info: '#7dd3fc',
          success: '#a7f3d0',
          warning: '#fef08a',
          error: '#fca5a5',
        },
      },
    ],
  },
  theme: { extend: {} },
  plugins: [require('daisyui'), require('@tailwindcss/forms')],
};
