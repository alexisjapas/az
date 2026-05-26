import { invoke } from "@tauri-apps/api/core";

export type AuthStatus = {
  db_path: string;
  db_exists: boolean;
  salt_exists: boolean;
  unlocked: boolean;
};

export async function authStatus(): Promise<AuthStatus> {
  return await invoke<AuthStatus>("auth_status");
}

export async function authUnlock(password: string): Promise<void> {
  await invoke("auth_unlock", { password });
}

export async function authCreate(password: string, confirm: string): Promise<void> {
  await invoke("auth_create", { password, confirm });
}

export async function authLock(): Promise<void> {
  await invoke("auth_lock");
}
