// Native appearance subscriptions are owned by Unistyles, without browser lifecycle events.
export function subscribeToSystemTheme(): () => void {
  return () => {};
}
