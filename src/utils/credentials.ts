import { invoke } from "@tauri-apps/api/core";

/**
 * Store a password securely in the OS keychain.
 * On macOS this uses Keychain, on Windows the Credential Manager,
 * and on Linux the Secret Service.
 */
export async function storeCredential(accountId: string, password: string): Promise<void> {
  await invoke("store_credential", { accountId, password });
}

/**
 * Retrieve a password from the OS keychain.
 * Returns null if the credential doesn't exist.
 */
export async function getCredential(accountId: string): Promise<string | null> {
  return await invoke<string | null>("get_credential", { accountId });
}

/**
 * Delete a password from the OS keychain.
 */
export async function deleteCredential(accountId: string): Promise<void> {
  await invoke("delete_credential", { accountId });
}

/**
 * Check if credentials API is available (Tauri backend running).
 */
export async function isCredentialStoreAvailable(): Promise<boolean> {
  try {
    await invoke("get_credential", { accountId: "__test__" });
    return true;
  } catch {
    return false;
  }
}
