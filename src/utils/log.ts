/**
 * Application logger — wraps console methods so ESLint no-console
 * doesn't fire throughout the codebase. In production builds these
 * could be routed to a file or remote service.
 */

/* eslint-disable no-console */
export const log = {
  info: (...args: unknown[]) => console.log(...args),
  warn: (...args: unknown[]) => console.warn(...args),
  error: (...args: unknown[]) => console.error(...args),
  debug: (...args: unknown[]) => console.debug(...args),
};
/* eslint-enable no-console */
