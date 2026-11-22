// Light or dark: the saved choice, else the system's (index.html applies it
// before the first paint).
export type Theme = 'light' | 'dark';

class ThemeState {
  current = $state<Theme>(document.documentElement.dataset.theme === 'dark' ? 'dark' : 'light');

  toggle(): void {
    this.current = this.current === 'dark' ? 'light' : 'dark';
    document.documentElement.dataset.theme = this.current;
    try {
      localStorage.setItem('lucy-theme', this.current);
    } catch {
      // Storage off: the choice lasts until the page reloads.
    }
  }
}

export const theme = new ThemeState();
