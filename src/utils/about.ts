/**
 * Links and labels for the About dialog. Kept free of React and Tauri so the
 * URL shapes are unit-tested: a wrong tag format sends every user to a 404.
 */

const REPO_URL = "https://github.com/5060-Solutions/aria";

export const WEBSITE_URL = "https://aria-softphone.com";

/**
 * Third-party components, from GitHub's dependency graph of the public repo:
 * every npm and Cargo dependency with its license, kept current by GitHub
 * rather than by a list in the app that would drift.
 */
export const ACKNOWLEDGEMENTS_URL = `${REPO_URL}/network/dependencies`;

/** What the backend's `app_build_info` command returns. */
export interface BuildInfo {
  commit: string | null;
  os: string;
  arch: string;
  release: boolean;
}

/** Release tags are `v` + the version (see .github/workflows/release.yml). */
function tagFor(version: string): string {
  return `v${version.replace(/^v/, "")}`;
}

/** The GitHub release for this version: its notes and downloads. */
export function releaseNotesUrl(version: string): string {
  return `${REPO_URL}/releases/tag/${encodeURIComponent(tagFor(version))}`;
}

/** The license as it was at this version's tag, not whatever main says now. */
export function licenseUrl(version: string | null): string {
  const ref = version ? encodeURIComponent(tagFor(version)) : "main";
  return `${REPO_URL}/blob/${ref}/LICENSE`;
}

const OS_LABELS: Record<string, string> = {
  macos: "macOS",
  windows: "Windows",
  linux: "Linux",
};

const ARCH_LABELS: Record<string, string> = {
  aarch64: "arm64",
  x86_64: "x64",
  x86: "x86",
};

/** "macOS · arm64", from Rust's `std::env::consts` names. */
export function platformLabel(info: Pick<BuildInfo, "os" | "arch">): string {
  const os = OS_LABELS[info.os] ?? info.os;
  const arch = ARCH_LABELS[info.arch] ?? info.arch;
  return `${os} · ${arch}`;
}

/**
 * One line that says exactly which build this is, for pasting into a bug
 * report: "Aria 0.6.3 (4e51409abc) macOS arm64".
 */
export function versionDetails(version: string, info: BuildInfo | null): string {
  const parts = [`Aria ${version}`];
  if (info?.commit) parts.push(`(${info.commit})`);
  if (info) {
    parts.push(`${OS_LABELS[info.os] ?? info.os} ${ARCH_LABELS[info.arch] ?? info.arch}`);
    if (!info.release) parts.push("debug");
  }
  return parts.join(" ");
}
