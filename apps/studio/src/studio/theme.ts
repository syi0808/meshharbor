export type StudioTheme = 'dark' | 'light';

const THEME_KEY = 'meshharbor.studio.theme.v1';

export function loadStudioTheme(): StudioTheme {
  try {
    return window.localStorage.getItem(THEME_KEY) === 'light' ? 'light' : 'dark';
  } catch {
    return 'dark';
  }
}

export function applyStudioTheme(theme: StudioTheme) {
  document.documentElement.classList.toggle('dark', theme === 'dark');
  document.documentElement.classList.toggle('scheme-dark', theme === 'dark');
  document.documentElement.classList.toggle('scheme-light', theme === 'light');
  document.querySelector('meta[name="theme-color"]')?.setAttribute('content', theme === 'dark' ? '#0a0a0a' : '#ffffff');
  try {
    window.localStorage.setItem(THEME_KEY, theme);
  } catch {
    return;
  }
}
