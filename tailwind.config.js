/** @type {import('tailwindcss').Config} */
module.exports = {
  content: [
    "./templates/**/*.html",
    "./static/js/**/*.js",
  ],
  theme: {
    extend: {
      colors: {
        seafile: {
          blue: "#2196f3",
          dark: "#1a2634",
          sidebar: "#1e2a3a",
        },
      },
    },
  },
  plugins: [],
};
