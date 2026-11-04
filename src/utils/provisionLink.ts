/**
 * One-click setup links: `aria://provision?v=1&url=<percent-encoded redeem URL>`.
 *
 * The redeem URL's last path segment is a one-time token, so neither the link
 * nor the URL is ever logged — only the `reason` a link was refused, which
 * never includes either.
 *
 * Any web page can fire an `aria://` link. Parsing one only leads to a preview
 * and a confirm dialog; nothing is redeemed without the user's click. The
 * backend checks the URL again before fetching it.
 */

export interface ProvisionLink {
  /** The redeem URL, normalised. */
  redeemUrl: string;
  /** `host[:port]` of the redeem URL: who is setting the line up. */
  serverHost: string;
}

export type ProvisionLinkResult =
  | { ok: true; link: ProvisionLink }
  | { ok: false; reason: string };

/**
 * Whether this build accepts setup links to plain-http localhost: dev builds,
 * or a build made with `VITE_ARIA_ALLOW_LOCAL_SETUP_LINKS=1`. The backend
 * refuses them anyway unless it is a debug build.
 */
export const allowLocalSetupLinks =
  import.meta.env.DEV || import.meta.env.VITE_ARIA_ALLOW_LOCAL_SETUP_LINKS === "1";

const PATH_PREFIX = "/provision/softphone/";
const LINK_PREFIX = /^aria:\/\/provision\/?(?=[?#]|$)/i;

/** Whether a link is an `aria://provision` link at all (any version, valid or not). */
export function isProvisionLink(link: string): boolean {
  return LINK_PREFIX.test(link.trim());
}

/**
 * Check a setup link against the contract.
 *
 * `allowLocalHttp` permits plain `http` to `localhost`/`127.0.0.1`, for dev
 * builds testing against a local stub. Never throws.
 */
export function parseProvisionLink(
  link: string,
  { allowLocalHttp = false }: { allowLocalHttp?: boolean } = {},
): ProvisionLinkResult {
  const trimmed = link.trim();
  const prefix = LINK_PREFIX.exec(trimmed);
  if (!prefix) return { ok: false, reason: "not an aria://provision link" };

  const rest = trimmed.slice(prefix[0].length);
  const query = rest.startsWith("?") ? rest.slice(1).split("#")[0] : "";
  // URLSearchParams leaves a bad percent sequence as-is rather than throwing.
  const params = new URLSearchParams(query);

  const versions = params.getAll("v");
  if (versions.length !== 1 || versions[0] !== "1") {
    return { ok: false, reason: "unsupported version" };
  }
  const urls = params.getAll("url");
  if (urls.length !== 1 || !urls[0]) return { ok: false, reason: "missing url" };

  let url: URL;
  try {
    url = new URL(urls[0]);
  } catch {
    return { ok: false, reason: "url does not parse" };
  }

  const localHost = url.hostname === "localhost" || url.hostname === "127.0.0.1";
  if (url.protocol !== "https:" && !(url.protocol === "http:" && allowLocalHttp && localHost)) {
    return { ok: false, reason: "url is not https" };
  }
  if (url.username || url.password) return { ok: false, reason: "url has userinfo" };
  if (!url.hostname) return { ok: false, reason: "url has no host" };
  if (!url.pathname.startsWith(PATH_PREFIX) || url.pathname.length === PATH_PREFIX.length) {
    return { ok: false, reason: "url has the wrong path" };
  }

  return { ok: true, link: { redeemUrl: url.href, serverHost: url.host } };
}
