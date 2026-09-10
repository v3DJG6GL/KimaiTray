import { invoke } from "@tauri-apps/api/core";

export async function getIdleSeconds(): Promise<number> {
  return invoke<number>("get_idle_seconds");
}

export interface IdlePeriod {
  /** Unix timestamps in seconds. */
  startedAt: number;
  endedAt: number;
}

export async function getIdleStats(): Promise<IdlePeriod[]> {
  return invoke<IdlePeriod[]>("get_idle_stats");
}
