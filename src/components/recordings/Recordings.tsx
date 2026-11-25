import { useEffect, useMemo, useState } from "react";
import {
  Avatar,
  Box,
  Button,
  Chip,
  CircularProgress,
  IconButton,
  List,
  ListItemAvatar,
  ListItemButton,
  ListItemText,
  Tooltip,
  Typography,
  alpha,
  useTheme,
} from "@mui/material";
import GraphicEqIcon from "@mui/icons-material/GraphicEq";
import PlayArrowIcon from "@mui/icons-material/PlayArrow";
import FolderOpenIcon from "@mui/icons-material/FolderOpen";
import CallMadeIcon from "@mui/icons-material/CallMade";
import CallReceivedIcon from "@mui/icons-material/CallReceived";
import NotesIcon from "@mui/icons-material/Notes";
import { invoke } from "@tauri-apps/api/core";
import { useTranslation } from "react-i18next";
import { useAppStore } from "../../stores/appStore";
import { errorText, notifyError } from "../../stores/toastStore";
import { playRecording } from "../../hooks/useSip";
import { recordedCalls, transcriptFor, type TranscriptSummary } from "../../utils/recordings";
import { log } from "../../utils/log";
import type { CallHistoryEntry } from "../../types/sip";

interface AiModel {
  id: string;
  kind: string;
  installed: boolean;
}

interface AiSegment {
  speaker: string;
  startMs: number;
  endMs: number;
  text: string;
}

interface AiInsight {
  callId: string;
  createdAt: number;
  durationSecs: number;
  segments: AiSegment[];
  language: string | null;
  summaryHeadline: string | null;
  summaryPoints: string[];
  status: string;
  narrowband: boolean;
}

function formatDuration(seconds: number): string {
  const m = Math.floor(seconds / 60);
  const s = seconds % 60;
  return `${m}:${s.toString().padStart(2, "0")}`;
}

function formatWhen(timestamp: number): string {
  const date = new Date(timestamp);
  const today = date.toDateString() === new Date().toDateString();
  const time = date.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  return today ? time : `${date.toLocaleDateString([], { month: "short", day: "numeric" })} · ${time}`;
}

function callerOf(entry: CallHistoryEntry): string {
  return entry.remoteName || entry.remoteUri.replace(/^sips?:/i, "").split("@")[0];
}

function clock(ms: number): string {
  const s = Math.floor(ms / 1000);
  return `${Math.floor(s / 60)}:${(s % 60).toString().padStart(2, "0")}`;
}

/**
 * Every recorded call, newest first, with its transcript when transcription
 * is on: the place recordings and transcripts live, instead of a line in a
 * history entry and a list at the bottom of Settings.
 */
export function Recordings() {
  const { t } = useTranslation();
  const theme = useTheme();
  const callHistory = useAppStore((s) => s.callHistory);
  const setCurrentView = useAppStore((s) => s.setCurrentView);
  const calls = useMemo(() => recordedCalls(callHistory), [callHistory]);

  const [aiAvailable, setAiAvailable] = useState<boolean | null>(null);
  const [hasModel, setHasModel] = useState(false);
  const [insights, setInsights] = useState<TranscriptSummary[]>([]);
  const [openId, setOpenId] = useState<string | null>(null);
  const [transcript, setTranscript] = useState<AiInsight | null>(null);
  const [busy, setBusy] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    invoke<boolean>("ai_available")
      .catch(() => false)
      .then(async (available) => {
        if (!live) return;
        setAiAvailable(available);
        if (!available) return;
        const [models, rows] = await Promise.all([
          invoke<AiModel[]>("ai_models").catch(() => [] as AiModel[]),
          invoke<TranscriptSummary[]>("ai_insights", { limit: 200, offset: 0 }).catch(() => [] as TranscriptSummary[]),
        ]);
        if (!live) return;
        setHasModel(models.some((m) => m.kind === "stt" && m.installed));
        setInsights(rows);
      });
    return () => {
      live = false;
    };
  }, []);

  const toggle = async (entry: CallHistoryEntry, summary: TranscriptSummary | null) => {
    if (openId === entry.id) {
      setOpenId(null);
      setTranscript(null);
      return;
    }
    setOpenId(entry.id);
    setTranscript(null);
    if (summary) {
      try {
        setTranscript(await invoke<AiInsight | null>("ai_insight", { callId: summary.callId }));
      } catch (e) {
        log.warn("[recordings] transcript load failed:", e);
      }
    }
  };

  const transcribe = async (entry: CallHistoryEntry) => {
    if (!entry.recordingPath) return;
    setBusy(entry.id);
    try {
      const insight = await invoke<AiInsight>("ai_transcribe_recording", {
        callId: entry.id,
        wavPath: entry.recordingPath,
        remoteLabel: callerOf(entry),
      });
      setInsights((rows) => [
        { callId: insight.callId, createdAt: insight.createdAt, durationSecs: insight.durationSecs, status: insight.status },
        ...rows.filter((r) => r.callId !== insight.callId),
      ]);
      setOpenId(entry.id);
      setTranscript(insight);
    } catch (e) {
      notifyError(t("recordings.failed", { error: errorText(e) }));
    } finally {
      setBusy(null);
    }
  };

  const hint =
    aiAvailable === false ? t("recordings.notBuilt") : aiAvailable && !hasModel ? t("recordings.noModel") : null;

  return (
    <Box sx={{ height: "100%", display: "flex", flexDirection: "column" }}>
      <Box sx={{ display: "flex", alignItems: "center", gap: 1, px: 2, pt: 2, pb: 1 }}>
        <GraphicEqIcon sx={{ color: "primary.main" }} />
        <Typography variant="h6" sx={{ fontWeight: 600, flex: 1 }}>
          {t("recordings.title")}
        </Typography>
        {calls.length > 0 && <Chip size="small" label={t("recordings.count", { count: calls.length })} />}
        <Tooltip title={t("recordings.openFolder")}>
          <IconButton size="small" onClick={() => invoke("open_recordings_folder").catch((e) => log.error("[recordings] open folder:", e))} aria-label={t("recordings.openFolder")}>
            <FolderOpenIcon fontSize="small" />
          </IconButton>
        </Tooltip>
      </Box>
      {hint && (
        <Box sx={{ px: 2, pb: 1 }}>
          <Typography variant="caption" sx={{ color: "text.secondary" }}>
            {hint}{" "}
            {aiAvailable && !hasModel && (
              <Box component="span" onClick={() => setCurrentView("settings")} sx={{ color: "primary.main", cursor: "pointer" }} role="link" tabIndex={0}>
                {t("nav.settings")}
              </Box>
            )}
          </Typography>
        </Box>
      )}
      {calls.length === 0 ? (
        <Box sx={{ flex: 1, display: "flex", alignItems: "center", justifyContent: "center", px: 3, textAlign: "center", color: "text.secondary" }}>
          <Typography variant="body2">{t("recordings.empty")}</Typography>
        </Box>
      ) : (
        <List sx={{ flex: 1, overflow: "auto", px: 1 }}>
          {calls.map((entry) => {
            const summary = transcriptFor(insights, entry);
            const open = openId === entry.id;
            const label = callerOf(entry);
            return (
              <Box key={entry.id}>
                <ListItemButton onClick={() => void toggle(entry, summary)} sx={{ borderRadius: "16px", mb: 0.5, py: 1, alignItems: "flex-start" }} selected={open}>
                  <ListItemAvatar>
                    <Avatar sx={{ width: 40, height: 40, bgcolor: alpha(theme.palette.primary.main, 0.1), color: "primary.main", fontSize: "0.9rem" }}>
                      {label.substring(0, 2).toUpperCase()}
                    </Avatar>
                  </ListItemAvatar>
                  <ListItemText
                    primary={
                      <Box sx={{ display: "flex", alignItems: "center", gap: 0.75 }}>
                        {entry.direction === "outbound" ? <CallMadeIcon sx={{ fontSize: 14, color: "text.secondary" }} /> : <CallReceivedIcon sx={{ fontSize: 14, color: "text.secondary" }} />}
                        <span style={{ fontWeight: 500 }}>{label}</span>
                        {summary && <NotesIcon sx={{ fontSize: 14, color: "primary.main" }} titleAccess={t("recordings.transcript")} />}
                      </Box>
                    }
                    secondary={
                      <Box component="span" sx={{ display: "flex", gap: 1, fontSize: "0.8rem" }}>
                        <span>{formatWhen(entry.startTime)}</span>
                        <span>&middot; {formatDuration(entry.duration)}</span>
                      </Box>
                    }
                  />
                  <Tooltip title={t("recordings.play")}>
                    <IconButton
                      size="small"
                      onClick={(e) => {
                        e.stopPropagation();
                        playRecording(entry.recordingPath ?? "").catch((err) => notifyError(errorText(err)));
                      }}
                      sx={{ bgcolor: alpha(theme.palette.primary.main, 0.1), color: "primary.main", mt: 0.5 }}
                      aria-label={t("recordings.play")}
                    >
                      <PlayArrowIcon fontSize="small" />
                    </IconButton>
                  </Tooltip>
                </ListItemButton>
                {open && (
                  <Box sx={{ mx: 2, mb: 1.5, p: 1.5, borderRadius: "12px", bgcolor: alpha(theme.palette.text.primary, 0.03) }}>
                    {summary ? (
                      transcript ? (
                        <>
                          {transcript.narrowband && (
                            <Typography variant="caption" sx={{ color: "text.disabled", display: "block", mb: 1 }}>
                              {t("transcription.narrowband")}
                            </Typography>
                          )}
                          {transcript.summaryHeadline && (
                            <Typography variant="subtitle2" sx={{ mb: 0.5 }}>
                              {transcript.summaryHeadline}
                            </Typography>
                          )}
                          {transcript.summaryPoints.length > 0 && (
                            <Box component="ul" sx={{ m: 0, mb: 1, pl: 2.5 }}>
                              {transcript.summaryPoints.map((p, i) => (
                                <Typography key={i} component="li" variant="body2">{p}</Typography>
                              ))}
                            </Box>
                          )}
                          {transcript.segments.length === 0 ? (
                            <Typography variant="body2" sx={{ color: "text.disabled" }}>{t("transcription.noSpeech")}</Typography>
                          ) : (
                            transcript.segments.map((seg, i) => (
                              <Box key={i} sx={{ display: "flex", gap: 1, mb: 0.5 }}>
                                <Typography variant="caption" sx={{ fontFamily: "monospace", color: "text.disabled", minWidth: 36, pt: 0.2 }}>{clock(seg.startMs)}</Typography>
                                <Typography variant="body2">
                                  <Box component="span" sx={{ fontWeight: 600, color: seg.speaker === "local" ? "primary.main" : "text.secondary", mr: 0.5 }}>
                                    {seg.speaker === "local" ? t("recordings.you") : t("recordings.caller")}
                                  </Box>
                                  {seg.text}
                                </Typography>
                              </Box>
                            ))
                          )}
                        </>
                      ) : (
                        <CircularProgress size={18} />
                      )
                    ) : aiAvailable && hasModel ? (
                      <Button size="small" variant="outlined" startIcon={busy === entry.id ? <CircularProgress size={14} /> : <NotesIcon />} disabled={busy === entry.id} onClick={() => void transcribe(entry)}>
                        {busy === entry.id ? t("recordings.transcribing") : t("recordings.transcribe")}
                      </Button>
                    ) : (
                      <Typography variant="caption" sx={{ color: "text.disabled" }}>{hint ?? t("recordings.noTranscript")}</Typography>
                    )}
                  </Box>
                )}
              </Box>
            );
          })}
        </List>
      )}
    </Box>
  );
}
