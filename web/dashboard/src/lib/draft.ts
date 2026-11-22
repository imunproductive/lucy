// A form's draft of server values: what the user changed stays when a
// refresh brings new values, and only the changed fields are sent.

/** A draft of `fields` from the server's values. */
export function draftOf<T extends object, K extends keyof T & string>(base: T, fields: readonly K[]): Pick<T, K> {
  const draft = {} as Pick<T, K>;
  for (const field of fields) draft[field] = base[field];
  return draft;
}

/**
 * Move a draft from `before` to `after` (a refresh): fields the user did not
 * change take the new value, changed ones keep what was typed.
 */
export function rebase<T extends object, K extends keyof T & string>(draft: Pick<T, K>, before: T, after: T, fields: readonly K[]): void {
  for (const field of fields) {
    if (same(draft[field], before[field])) draft[field] = after[field];
  }
}

/** The fields whose draft differs from the server's value. */
export function changes<T extends object, K extends keyof T & string>(draft: Pick<T, K>, base: T, fields: readonly K[]): Partial<Pick<T, K>> {
  const changed: Partial<Pick<T, K>> = {};
  for (const field of fields) {
    if (!same(draft[field], base[field])) changed[field] = draft[field];
  }
  return changed;
}

function same(a: unknown, b: unknown): boolean {
  if (typeof a === 'number' || typeof b === 'number') return Number(a) === Number(b);
  if (typeof a === 'boolean' || typeof b === 'boolean') return Boolean(a) === Boolean(b);
  return (a ?? '') === (b ?? '');
}
