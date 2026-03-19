import { useState, useEffect, useRef } from "react";
import { Box, Typography, alpha, useTheme } from "@mui/material";
import MicIcon from "@mui/icons-material/Mic";
import VolumeUpIcon from "@mui/icons-material/VolumeUp";
import { invoke } from "@tauri-apps/api/core";

interface AudioLevels {
  tx: number;
  rx: number;
}

function LevelBar({ level, color }: { level: number; color: string }) {
  // Convert RMS to a more visual-friendly scale (RMS is typically 0-0.3 for speech)
  const displayLevel = Math.min(level * 3.3, 1.0);
  const barCount = 20;
  const activeBars = Math.round(displayLevel * barCount);

  return (
    <Box sx={{ display: "flex", gap: "2px", alignItems: "center", flex: 1 }}>
      {Array.from({ length: barCount }, (_, i) => (
        <Box
          key={i}
          sx={{
            width: "100%",
            height: 6,
            borderRadius: 1,
            bgcolor: i < activeBars
              ? i > barCount * 0.8
                ? "#ef4444" // red for hot
                : i > barCount * 0.6
                  ? "#f59e0b" // amber for warm
                  : color
              : alpha(color, 0.12),
            transition: "background-color 80ms ease",
          }}
        />
      ))}
    </Box>
  );
}

export function AudioLevelMeter({ compact }: { compact?: boolean }) {
  const theme = useTheme();
  const [levels, setLevels] = useState<AudioLevels | null>(null);
  const intervalRef = useRef<ReturnType<typeof setInterval> | null>(null);

  useEffect(() => {
    const poll = async () => {
      try {
        const result = await invoke<AudioLevels | null>("get_audio_levels");
        setLevels(result);
      } catch {
        // no active call
        setLevels(null);
      }
    };

    poll();
    intervalRef.current = setInterval(poll, 80);

    return () => {
      if (intervalRef.current) clearInterval(intervalRef.current);
    };
  }, []);

  if (!levels) return null;

  if (compact) {
    return (
      <Box sx={{ display: "flex", flexDirection: "column", gap: 0.5, width: "100%" }}>
        <Box sx={{ display: "flex", alignItems: "center", gap: 1 }}>
          <MicIcon sx={{ fontSize: 14, color: "text.secondary" }} />
          <LevelBar level={levels.tx} color={theme.palette.primary.main} />
        </Box>
        <Box sx={{ display: "flex", alignItems: "center", gap: 1 }}>
          <VolumeUpIcon sx={{ fontSize: 14, color: "text.secondary" }} />
          <LevelBar level={levels.rx} color={theme.palette.success.main} />
        </Box>
      </Box>
    );
  }

  return (
    <Box
      sx={{
        display: "flex",
        flexDirection: "column",
        gap: 1,
        p: 1.5,
        borderRadius: "12px",
        bgcolor: alpha(theme.palette.text.primary, 0.03),
        border: `1px solid ${alpha(theme.palette.divider, 0.08)}`,
      }}
    >
      <Typography
        variant="caption"
        sx={{ fontSize: "0.65rem", fontWeight: 600, color: "text.secondary", letterSpacing: 1 }}
      >
        AUDIO LEVELS
      </Typography>
      <Box sx={{ display: "flex", alignItems: "center", gap: 1 }}>
        <MicIcon sx={{ fontSize: 16, color: "text.secondary" }} />
        <Typography variant="caption" sx={{ fontSize: "0.65rem", color: "text.secondary", minWidth: 20 }}>
          TX
        </Typography>
        <LevelBar level={levels.tx} color={theme.palette.primary.main} />
      </Box>
      <Box sx={{ display: "flex", alignItems: "center", gap: 1 }}>
        <VolumeUpIcon sx={{ fontSize: 16, color: "text.secondary" }} />
        <Typography variant="caption" sx={{ fontSize: "0.65rem", color: "text.secondary", minWidth: 20 }}>
          RX
        </Typography>
        <LevelBar level={levels.rx} color={theme.palette.success.main} />
      </Box>
    </Box>
  );
}
