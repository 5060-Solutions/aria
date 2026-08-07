import { useState, useEffect, useCallback } from "react";
import {
  Box,
  Typography,
  Switch,
  Button,
  LinearProgress,
  CircularProgress,
  IconButton,
  Dialog,
  DialogTitle,
  DialogContent,
  DialogActions,
} from "@mui/material";
import DeleteIcon from "@mui/icons-material/Delete";
import DownloadIcon from "@mui/icons-material/Download";
import CloseIcon from "@mui/icons-material/Close";
import { invoke } from "@tauri-apps/api/core";
import { useTranslation } from "react-i18next";

interface AiModel {
  id: string;
  displayName: string;
  kind: string;
  sizeBytes: number;
  available: boolean;
  unavailableReason: string | null;
  installed: boolean;
}

interface AiDownloadProgress {
  modelId: string;
  downloadedBytes: number;
  totalBytes: number;
  state: string;
  error: string | null;
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

interface AiInsightSummary {
  callId: string;
  createdAt: number;
  durationSecs: number;
  status: string;
}

function humanBytes(n: number): string {
  if (n >= 1_000_000_000) return `${(n / 1_000_000_000).toFixed(1)} GB`;
  if (n >= 1_000_000) return `${Math.round(n / 1_000_000)} MB`;
  return `${Math.round(n / 1000)} KB`;
}

function formatTimestamp(epochSeconds: number): string {
  return new Date(epochSeconds * 1000).toLocaleString();
}

/**
 * On-device transcription: manage models, opt in, and read past transcripts.
 *
 * Transcription runs on this machine against the call recording that is already
 * written to disk, so nothing is uploaded and nothing extra happens during a
 * call.
 */
export function Transcription() {
  const { t } = useTranslation();

  const [supported, setSupported] = useState<boolean | null>(null);
  const [models, setModels] = useState<AiModel[]>([]);
  const [progress, setProgress] = useState<Record<string, AiDownloadProgress>>({});
  const [autoTranscribe, setAutoTranscribe] = useState(false);
  const [insights, setInsights] = useState<AiInsightSummary[]>([]);
  const [open, setOpen] = useState<AiInsight | null>(null);
  const [error, setError] = useState<string | null>(null);

  const sttInstalled = models.some((m) => m.kind === "stt" && m.installed);

  const refresh = useCallback(async () => {
    try {
      const [m, a, i] = await Promise.all([
        invoke<AiModel[]>("ai_models"),
        invoke<boolean>("ai_auto_transcribe"),
        invoke<AiInsightSummary[]>("ai_insights", { limit: 100, offset: 0 }),
      ]);
      setModels(m);
      setAutoTranscribe(a);
      setInsights(i);
    } catch (e) {
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    invoke<boolean>("ai_available")
      .then((ok) => {
        setSupported(ok);
        if (ok) void refresh();
      })
      .catch(() => setSupported(false));
  }, [refresh]);

  // Downloads report progress by polling rather than pushing, so this is the
  // UI's own timer. It stops as soon as nothing is in flight.
  useEffect(() => {
    if (!supported) return;
    const pending = models.filter((m) => !m.installed);
    if (pending.length === 0) return;

    const timer = setInterval(() => {
      void (async () => {
        const next: Record<string, AiDownloadProgress> = {};
        let anyCompleted = false;
        for (const m of pending) {
          const p = await invoke<AiDownloadProgress | null>("ai_download_progress", {
            modelId: m.id,
          }).catch(() => null);
          if (p) {
            next[m.id] = p;
            if (p.state === "completed") anyCompleted = true;
          }
        }
        setProgress(next);
        if (anyCompleted) void refresh();
      })();
    }, 1000);
    return () => clearInterval(timer);
  }, [supported, models, refresh]);

  const toggleAuto = async (on: boolean) => {
    setAutoTranscribe(on);
    try {
      await invoke("ai_set_auto_transcribe", { enabled: on });
    } catch (e) {
      setError(String(e));
      setAutoTranscribe(!on);
    }
  };

  const deleteModel = async (id: string) => {
    try {
      await invoke("ai_delete_model", { modelId: id });
      const m = await invoke<AiModel[]>("ai_models");
      setModels(m);
      // Without a speech model there is nothing to transcribe with, so leaving
      // the switch on would be a promise the app cannot keep.
      if (!m.some((x) => x.kind === "stt" && x.installed) && autoTranscribe) {
        await toggleAuto(false);
      }
    } catch (e) {
      setError(String(e));
    }
  };

  if (supported === null) {
    return <CircularProgress size={24} sx={{ m: 3 }} />;
  }

  if (!supported) {
    return (
      <Box sx={{ p: 2 }}>
        <Typography variant="body2" sx={{ color: "text.secondary" }}>
          {t("transcription.unsupported")}
        </Typography>
      </Box>
    );
  }

  return (
    <Box sx={{ display: "flex", flexDirection: "column", gap: 2, p: 1 }}>
      <Typography variant="body2" sx={{ color: "text.secondary" }}>
        {t("transcription.intro")}
      </Typography>

      {error && (
        <Typography variant="body2" color="error">
          {error}
        </Typography>
      )}

      {/* Transcribing a recording is a separate decision from making one, so
          this is its own opt-in rather than riding on auto-record. */}
      <Box sx={{ display: "flex", alignItems: "center", justifyContent: "space-between" }}>
        <Box>
          <Typography variant="body2">{t("transcription.autoTitle")}</Typography>
          <Typography variant="caption" sx={{ color: "text.disabled" }}>
            {sttInstalled ? t("transcription.autoNote") : t("transcription.autoNeedsModel")}
          </Typography>
        </Box>
        <Switch
          checked={autoTranscribe && sttInstalled}
          disabled={!sttInstalled}
          onChange={(e) => void toggleAuto(e.target.checked)}
          size="small"
        />
      </Box>

      <Typography variant="subtitle2">{t("transcription.models")}</Typography>
      {models.map((m) => {
        const p = progress[m.id];
        const downloading = p && (p.state === "downloading" || p.state === "queued" || p.state === "verifying");
        return (
          <Box key={m.id} sx={{ display: "flex", flexDirection: "column", gap: 0.5 }}>
            <Box sx={{ display: "flex", alignItems: "center", justifyContent: "space-between" }}>
              <Box>
                <Typography variant="body2">{m.displayName}</Typography>
                <Typography variant="caption" sx={{ color: "text.disabled" }}>
                  {(m.kind === "llm" ? t("transcription.kindSummaries") : t("transcription.kindSpeech")) +
                    " · " +
                    humanBytes(m.sizeBytes)}
                  {m.unavailableReason ? ` · ${m.unavailableReason}` : ""}
                </Typography>
              </Box>
              {m.installed ? (
                <IconButton size="small" onClick={() => void deleteModel(m.id)}>
                  <DeleteIcon fontSize="small" />
                </IconButton>
              ) : downloading ? (
                <Button
                  size="small"
                  onClick={() => void invoke("ai_cancel_download", { modelId: m.id })}
                >
                  {t("transcription.cancel")}
                </Button>
              ) : (
                <Button
                  size="small"
                  startIcon={<DownloadIcon />}
                  disabled={!m.available}
                  onClick={() => void invoke("ai_start_download", { modelId: m.id })}
                >
                  {t("transcription.download")}
                </Button>
              )}
            </Box>
            {downloading && p.totalBytes > 0 && (
              <LinearProgress
                variant="determinate"
                value={(p.downloadedBytes / p.totalBytes) * 100}
              />
            )}
            {p?.error && (
              <Typography variant="caption" color="error">
                {p.error}
              </Typography>
            )}
          </Box>
        );
      })}

      <Typography variant="subtitle2" sx={{ mt: 1 }}>
        {t("transcription.transcripts")}
      </Typography>
      {insights.length === 0 ? (
        <Typography variant="caption" sx={{ color: "text.disabled" }}>
          {t("transcription.noTranscripts")}
        </Typography>
      ) : (
        insights.map((row) => (
          <Box
            key={row.callId}
            sx={{ display: "flex", alignItems: "center", justifyContent: "space-between" }}
          >
            <Box
              sx={{ cursor: "pointer" }}
              onClick={() =>
                void invoke<AiInsight | null>("ai_insight", { callId: row.callId }).then(setOpen)
              }
            >
              <Typography variant="body2">{formatTimestamp(row.createdAt)}</Typography>
              <Typography variant="caption" sx={{ color: "text.disabled" }}>
                {row.durationSecs}s{row.status !== "complete" ? ` · ${row.status}` : ""}
              </Typography>
            </Box>
            <IconButton
              size="small"
              onClick={() =>
                void invoke("ai_delete_insight", { callId: row.callId }).then(() => refresh())
              }
            >
              <DeleteIcon fontSize="small" />
            </IconButton>
          </Box>
        ))
      )}

      <Dialog open={open !== null} onClose={() => setOpen(null)} fullWidth maxWidth="sm">
        <DialogTitle sx={{ display: "flex", alignItems: "center", justifyContent: "space-between" }}>
          {open ? formatTimestamp(open.createdAt) : ""}
          <IconButton size="small" onClick={() => setOpen(null)}>
            <CloseIcon fontSize="small" />
          </IconButton>
        </DialogTitle>
        <DialogContent dividers>
          {/* An 8 kHz call is materially less accurate, and the user has to be
              told rather than left to wonder why the text is rough. */}
          {open?.narrowband && (
            <Typography variant="caption" sx={{ color: "text.disabled", display: "block", mb: 1 }}>
              {t("transcription.narrowband")}
            </Typography>
          )}
          {open?.summaryHeadline && (
            <Box sx={{ mb: 2 }}>
              <Typography variant="body2" sx={{ fontWeight: 600 }}>
                {open.summaryHeadline}
              </Typography>
              {open.summaryPoints.map((pt, i) => (
                <Typography key={i} variant="body2">
                  • {pt}
                </Typography>
              ))}
            </Box>
          )}
          {open?.segments.length === 0 && (
            <Typography variant="body2" sx={{ color: "text.secondary" }}>
              {t("transcription.noSpeech")}
            </Typography>
          )}
          {open?.segments.map((s, i) => (
            <Typography key={i} variant="body2" sx={{ mb: 0.5 }}>
              {s.text}
            </Typography>
          ))}
        </DialogContent>
        <DialogActions>
          <Button onClick={() => setOpen(null)}>{t("transcription.close")}</Button>
        </DialogActions>
      </Dialog>
    </Box>
  );
}
