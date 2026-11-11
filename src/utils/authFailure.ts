/**
 * Whether a registration error is a rejected login. Those are not retried on
 * their own: the password will not become right by waiting.
 */
export function isAuthFailure(error: string | null | undefined): boolean {
  return !!error && /authenticat|\b40[13]\b/i.test(error);
}
