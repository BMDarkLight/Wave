import { useState, useEffect } from "react";
import {
  playTrack,
  pauseTrack,
  resumeTrack,
  stopTrack,
  getPlaybackState,
  selectAudioFile,
  getFileName,
  addTrackToPlaylist,
  removeTrackFromPlaylist,
  getPlaylist,
  clearPlaylist,
  playTrackFromPlaylist,
  type PlaybackState,
  type Track,
} from "./utils/player";
import { invoke } from "@tauri-apps/api/core";
import "./App.css";

function App() {
  const [playbackState, setPlaybackState] = useState<PlaybackState>({
    is_playing: false,
    is_paused: false,
    current_path: null,
  });
  const [playlist, setPlaylist] = useState<Track[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [isLoading, setIsLoading] = useState(false);

  const updatePlaybackState = async () => {
    try {
      const state = await getPlaybackState();
      setPlaybackState(state);
      setError(null);
    } catch (err) {
      setError(
        err instanceof Error ? err.message : "Failed to get playback state"
      );
    }
  };

  const loadPlaylist = async () => {
    try {
      const tracks = await getPlaylist();
      setPlaylist(tracks);
    } catch (err) {
      console.error("Failed to load playlist:", err);
    }
  };

  useEffect(() => {
    console.log("App mounted, setting up intervals");
    
    // Wait for Tauri to be ready with retries
    const initApp = async () => {
      let retries = 10;
      let tauriReady = false;
      
      // Retry checking for Tauri
      while (retries > 0 && !tauriReady) {
        await new Promise(resolve => setTimeout(resolve, 200));
        
        // Try to call a Tauri command to see if it's available
        try {
          await getPlaybackState();
          tauriReady = true;
          console.log("✓ Tauri API is available");
        } catch (err: any) {
          // Check if it's a Tauri availability error or a real command error
          if (err?.message?.includes('not available') || err?.message?.includes('undefined')) {
            retries--;
            console.log(`Waiting for Tauri... (${retries} retries left)`);
          } else {
            // If it's a different error, Tauri is available but command failed
            tauriReady = true;
            console.log("✓ Tauri API is available (command error is expected)");
          }
        }
      }
      
      if (tauriReady) {
        try {
          await updatePlaybackState();
          await loadPlaylist();
          console.log("✓ App initialized successfully");
        } catch (err) {
          console.error("Error initializing app:", err);
          // Don't set error for initialization failures, just log them
        }
      } else {
        console.error("✗ Tauri not detected after retries");
        setError("Tauri API not available. Make sure you're running 'npm run tauri dev' and not just 'npm run dev'.");
      }
    };
    
    initApp();
    
    const interval = setInterval(() => {
      updatePlaybackState().catch(err => {
        // Silently fail if Tauri isn't ready yet
        if (!err?.message?.includes('not available')) {
          console.error("Error updating playback state:", err);
        }
      });
    }, 500);

    return () => {
      console.log("App unmounting, clearing intervals");
      clearInterval(interval);
    };
  }, []);

  const handleAddTrack = async (multiple: boolean = false) => {
    console.log("handleAddTrack called", { multiple });
    try {
      setError(null);
      setIsLoading(true);
      console.log("Opening file dialog...");
      const paths = await selectAudioFile(multiple);
      console.log("File dialog result:", paths);
      if (paths && paths.length > 0) {
        let successCount = 0;
        let failCount = 0;
        for (const path of paths) {
          try {
            console.log("Adding track to playlist:", path);
            await addTrackToPlaylist(path);
            successCount++;
            console.log("Track added successfully");
          } catch (err) {
            failCount++;
            console.error(`Failed to add track ${path}:`, err);
          }
        }
        if (failCount > 0) {
          setError(
            `Added ${successCount} track(s), failed to add ${failCount} track(s)`
          );
        }
        await loadPlaylist();
      } else {
        console.log("No files selected");
      }
    } catch (err) {
      const errorMessage =
        err instanceof Error ? err.message : "Failed to add track";
      setError(errorMessage);
      console.error("Error adding track:", err);
    } finally {
      setIsLoading(false);
    }
  };

  const handleRemoveTrack = async (index: number) => {
    try {
      setError(null);
      await removeTrackFromPlaylist(index);
      await loadPlaylist();
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to remove track");
    }
  };

  const handlePlayTrack = async (index: number) => {
    console.log("handlePlayTrack called", { index });
    try {
      setError(null);
      setIsLoading(true);
      console.log("Playing track from playlist at index:", index);
      await playTrackFromPlaylist(index);
      console.log("Track play command sent, updating state");
      await updatePlaybackState();
    } catch (err) {
      const errorMessage =
        err instanceof Error ? err.message : "Failed to play track";
      setError(errorMessage);
      console.error("Error playing track:", err);
    } finally {
      setIsLoading(false);
    }
  };

  const handlePlayPause = async () => {
    console.log("handlePlayPause called", { playbackState, playlistLength: playlist.length });
    try {
      setError(null);
      setIsLoading(true);
      if (playbackState.is_playing) {
        console.log("Pausing track");
        await pauseTrack();
      } else if (playbackState.is_paused) {
        console.log("Resuming track");
        await resumeTrack();
      } else if (playbackState.current_path) {
        console.log("Playing current path:", playbackState.current_path);
        await playTrack(playbackState.current_path);
      } else if (playlist.length > 0) {
        console.log("Playing first track from playlist");
        await handlePlayTrack(0);
        return;
      } else {
        console.log("No tracks, opening file dialog");
        await handleAddTrack(false);
        return;
      }
      await updatePlaybackState();
    } catch (err) {
      const errorMessage =
        err instanceof Error ? err.message : "Failed to control playback";
      setError(errorMessage);
      console.error("Error controlling playback:", err);
    } finally {
      setIsLoading(false);
    }
  };

  const handleStop = async () => {
    try {
      setError(null);
      await stopTrack();
      await updatePlaybackState();
    } catch (err) {
      const errorMessage =
        err instanceof Error ? err.message : "Failed to stop track";
      setError(errorMessage);
      console.error("Error stopping track:", err);
    }
  };

  const handleClearPlaylist = async () => {
    if (!confirm("Are you sure you want to clear the entire playlist?")) {
      return;
    }
    try {
      setError(null);
      setIsLoading(true);
      await clearPlaylist();
      await loadPlaylist();
    } catch (err) {
      const errorMessage =
        err instanceof Error ? err.message : "Failed to clear playlist";
      setError(errorMessage);
      console.error("Error clearing playlist:", err);
    } finally {
      setIsLoading(false);
    }
  };

  const currentTrackName = playbackState.current_path
    ? getFileName(playbackState.current_path)
    : "No track playing";

  const isCurrentTrack = (track: Track) =>
    track.path === playbackState.current_path;

  return (
    <div className="app-container">
      {/* Sidebar */}
      <aside className="sidebar">
        <div className="sidebar-header">
          <h1 className="logo">Wave</h1>
        </div>
        <nav className="sidebar-nav">
          <button className="nav-item active">
            <svg width="24" height="24" viewBox="0 0 24 24" fill="currentColor">
              <path d="M10 20v-6h4v6h5v-8h3L12 3 2 12h3v8z" />
            </svg>
            <span>Home</span>
          </button>
          <button className="nav-item">
            <svg width="24" height="24" viewBox="0 0 24 24" fill="currentColor">
              <path d="M12 3v10.55c-.59-.34-1.27-.55-2-.55-2.21 0-4 1.79-4 4s1.79 4 4 4 4-1.79 4-4V7h4V3h-6z" />
            </svg>
            <span>Search</span>
          </button>
        </nav>
        <div className="sidebar-playlist-section">
          <div className="playlist-header">
            <span>Your Library</span>
            <button
              className="icon-btn"
              onClick={(e) => {
                e.preventDefault();
                handleAddTrack(false);
              }}
              title="Add tracks"
            >
              <svg
                width="20"
                height="20"
                viewBox="0 0 24 24"
                fill="currentColor"
              >
                <path d="M19 13h-6v6h-2v-6H5v-2h6V5h2v6h6v2z" />
              </svg>
            </button>
          </div>
        </div>
      </aside>

      {/* Main Content */}
      <main className="main-content">
        <div className="content-header">
          <h2>Your Playlist</h2>
          <div className="header-actions">
            <button
              className="btn-secondary"
              onClick={(e) => {
                e.preventDefault();
                e.stopPropagation();
                console.log("Add Track button clicked");
                handleAddTrack(false);
              }}
              disabled={isLoading}
              type="button"
            >
              <svg
                width="16"
                height="16"
                viewBox="0 0 24 24"
                fill="currentColor"
              >
                <path d="M19 13h-6v6h-2v-6H5v-2h6V5h2v6h6v2z" />
              </svg>
              {isLoading ? "Adding..." : "Add Track"}
            </button>
            <button
              className="btn-secondary"
              onClick={(e) => {
                e.preventDefault();
                e.stopPropagation();
                console.log("Add Multiple button clicked");
                handleAddTrack(true);
              }}
              disabled={isLoading}
              type="button"
            >
              <svg
                width="16"
                height="16"
                viewBox="0 0 24 24"
                fill="currentColor"
              >
                <path d="M19 13h-6v6h-2v-6H5v-2h6V5h2v6h6v2z" />
              </svg>
              Add Multiple
            </button>
            {playlist.length > 0 && (
              <button 
                className="btn-secondary" 
                onClick={(e) => {
                  e.preventDefault();
                  e.stopPropagation();
                  console.log("Clear button clicked");
                  handleClearPlaylist();
                }}
                type="button"
              >
                Clear
              </button>
            )}
          </div>
        </div>

        {error && (
          <div className="error-banner">
            <span>{error}</span>
            <button
              className="error-close"
              onClick={() => setError(null)}
              title="Dismiss"
            >
              ×
            </button>
          </div>
        )}
        {isLoading && (
          <div className="loading-indicator">
            <div className="spinner"></div>
            <span>Loading...</span>
          </div>
        )}

        <div className="playlist-container">
          {playlist.length === 0 ? (
            <div className="empty-state">
              <svg
                width="64"
                height="64"
                viewBox="0 0 24 24"
                fill="currentColor"
                opacity="0.3"
              >
                <path d="M12 3v10.55c-.59-.34-1.27-.55-2-.55-2.21 0-4 1.79-4 4s1.79 4 4 4 4-1.79 4-4V7h4V3h-6z" />
              </svg>
              <p>Your playlist is empty</p>
              <button
                className="btn-primary"
                onClick={(e) => {
                  e.preventDefault();
                  handleAddTrack(false);
                }}
                disabled={isLoading}
              >
                {isLoading ? "Loading..." : "Add your first track"}
              </button>
            </div>
          ) : (
            <div className="track-list">
              <div className="track-list-header">
                <div className="track-col-index">#</div>
                <div className="track-col-title">Title</div>
                <div className="track-col-actions"></div>
              </div>
              {playlist.map((track, index) => (
                <div
                  key={`${track.path}-${index}`}
                  className={`track-item ${
                    isCurrentTrack(track) ? "active" : ""
                  }`}
                  onClick={(e) => {
                    e.preventDefault();
                    console.log("Track clicked:", index, track.name);
                    handlePlayTrack(index);
                  }}
                >
                  <div className="track-col-index">
                    {isCurrentTrack(track) && playbackState.is_playing ? (
                      <div className="playing-indicator">
                        <div className="wave-bar"></div>
                        <div className="wave-bar"></div>
                        <div className="wave-bar"></div>
                        <div className="wave-bar"></div>
                      </div>
                    ) : (
                      <span>{index + 1}</span>
                    )}
                  </div>
                  <div className="track-col-title">
                    <div className="track-name">{track.name}</div>
                  </div>
                  <div className="track-col-actions">
                    <button
                      className="track-action-btn"
                      onClick={(e) => {
                        e.stopPropagation();
                        handleRemoveTrack(index);
                      }}
                      title="Remove"
                    >
                      <svg
                        width="16"
                        height="16"
                        viewBox="0 0 24 24"
                        fill="currentColor"
                      >
                        <path d="M19 6.41L17.59 5 12 10.59 6.41 5 5 6.41 10.59 12 5 17.59 6.41 19 12 13.41 17.59 19 19 17.59 13.41 12z" />
                      </svg>
                    </button>
                  </div>
                </div>
              ))}
            </div>
          )}
        </div>
      </main>

      {/* Bottom Player Bar */}
      <footer className="player-bar">
        <div className="player-left">
          <div className="now-playing-info">
            <div className="now-playing-name">{currentTrackName}</div>
            <div className="now-playing-artist">Wave Player</div>
          </div>
        </div>
        <div className="player-center">
          <div className="player-controls">
            <button
              className="control-btn"
              onClick={(e) => {
                e.preventDefault();
                e.stopPropagation();
                console.log("Stop button clicked");
                handleStop();
              }}
              disabled={!playbackState.current_path}
              type="button"
              title="Stop"
            >
              <svg
                width="20"
                height="20"
                viewBox="0 0 24 24"
                fill="currentColor"
              >
                <rect x="6" y="6" width="12" height="12" />
              </svg>
            </button>
            <button
              className="control-btn play-pause-btn"
              onClick={(e) => {
                e.preventDefault();
                e.stopPropagation();
                console.log("Play/Pause button clicked");
                handlePlayPause();
              }}
              disabled={isLoading}
              type="button"
              title={
                playbackState.is_playing
                  ? "Pause"
                  : playbackState.is_paused
                  ? "Resume"
                  : "Play"
              }
            >
              {playbackState.is_playing ? (
                <svg
                  width="24"
                  height="24"
                  viewBox="0 0 24 24"
                  fill="currentColor"
                >
                  <rect x="6" y="4" width="4" height="16" />
                  <rect x="14" y="4" width="4" height="16" />
                </svg>
              ) : (
                <svg
                  width="24"
                  height="24"
                  viewBox="0 0 24 24"
                  fill="currentColor"
                >
                  <polygon points="5 3 19 12 5 21 5 3" />
                </svg>
              )}
            </button>
          </div>
        </div>
        <div className="player-right">
          <div className="player-status">
            {playbackState.is_playing && (
              <span className="status-badge playing">Playing</span>
            )}
            {playbackState.is_paused && (
              <span className="status-badge paused">Paused</span>
            )}
          </div>
        </div>
      </footer>
    </div>
  );
}

export default App;
