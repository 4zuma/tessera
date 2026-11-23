/** A deeply reactive props object for `mount()`, so a React re-render can update a Svelte component. */
export function reactive<T extends object>(initial: T): T {
  const p = $state(initial);
  return p;
}
