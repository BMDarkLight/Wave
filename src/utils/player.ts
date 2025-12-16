import { invoke } from "@tauri-apps/api/tauri";

export const playTrack = (path: string) => {
  return invoke("play_track", { path });
};