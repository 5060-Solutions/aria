/// <reference types="vite/client" />

interface ImportMetaEnv {
  /**
   * "1" lets setup links point at plain-http localhost, for testing a build
   * against a local stub. Only takes effect in a debug backend, which checks
   * the URL again.
   */
  readonly VITE_ARIA_ALLOW_LOCAL_SETUP_LINKS?: string;
}
