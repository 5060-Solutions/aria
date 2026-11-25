import { useEffect, useState } from "react";
import { Box, Button, Typography, alpha, useTheme } from "@mui/material";
import NotesIcon from "@mui/icons-material/Notes";
import { invoke } from "@tauri-apps/api/core";
import { useTranslation } from "react-i18next";
import { useAppStore } from "../../stores/appStore";
import type { CallHistoryEntry } from "../../types/sip";

interface AiSegment {
  speaker: string;
  startMs: number;
  text: string;
}

interface AiInsight {
  callId: string;
  segments: AiSegment[];
  summaryHeadline: string | null;
  summaryPoints: string[];
  narrowband: boolean;
}

/**
 * A call's transcript in its History detail: the summary when the build
 * makes one, the first lines otherwise, and the way to the full text in
 * Recordings. Renders nothing for a call without a transcript.
 */
export function CallTranscript({ entry }: { entry: CallHistoryEntry }) {
  const { t } = useTranslation();
  const theme = useTheme();
  const setCurrentView = useAppStore((s) => s.setCurrentView);
  const [insight, setInsight] = useState<AiInsight | null>(null);

  useEffect(() => {
    let live = true;
    const load = async () => {
      const available = await invoke<boolean>("ai_available").catch(() => false);
      if (!available || !live) return null;
      const own = await invoke<AiInsight | null>("ai_insight", { callId: entry.id }).catch(() => null);
      if (own || !entry.sipCallId) return own;
      return invoke<AiInsight | null>("ai_insight", { callId: entry.sipCallId }).catch(() => null);
    };
    load().then((i) => {
      if (live) setInsight(i);
    });
    return () => {
      live = false;
    };
  }, [entry.id, entry.sipCallId]);

  if (!insight) return null;
  const lines = insight.segments.slice(0, 3);
  return (
    <Box sx={{ display: "flex", gap: 2 }}>
      <NotesIcon sx={{ color: "primary.main" }} />
      <Box sx={{ flex: 1, minWidth: 0 }}>
        <Typography variant="body2" sx={{ fontWeight: 500 }}>
          {insight.summaryHeadline ?? t("history.transcript")}
        </Typography>
        {insight.summaryPoints.length > 0 ? (
          <Box component="ul" sx={{ m: 0, mt: 0.5, pl: 2.5 }}>
            {insight.summaryPoints.map((p, i) => (
              <Typography key={i} component="li" variant="caption" sx={{ color: "text.secondary", display: "list-item" }}>
                {p}
              </Typography>
            ))}
          </Box>
        ) : (
          <Box sx={{ mt: 0.5, p: 1, borderRadius: "8px", bgcolor: alpha(theme.palette.text.primary, 0.03) }}>
            {lines.length === 0 ? (
              <Typography variant="caption" sx={{ color: "text.disabled" }}>{t("transcription.noSpeech")}</Typography>
            ) : (
              lines.map((seg, i) => (
                <Typography key={i} variant="caption" sx={{ display: "block", color: "text.secondary" }} noWrap>
                  <Box component="span" sx={{ fontWeight: 600, mr: 0.5 }}>
                    {seg.speaker === "local" ? t("recordings.you") : t("recordings.caller")}
                  </Box>
                  {seg.text}
                </Typography>
              ))
            )}
          </Box>
        )}
        <Button size="small" onClick={() => setCurrentView("recordings")} sx={{ mt: 0.5, ml: -1, textTransform: "none" }}>
          {t("history.openInRecordings")}
        </Button>
      </Box>
    </Box>
  );
}
