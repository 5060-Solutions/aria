import { useState } from "react";
import { Box, Chip, IconButton, Tooltip, Typography, alpha, useTheme } from "@mui/material";
import ContentCopyIcon from "@mui/icons-material/ContentCopy";
import CheckIcon from "@mui/icons-material/Check";
import ArrowForwardIcon from "@mui/icons-material/ArrowForward";
import ArrowBackIcon from "@mui/icons-material/ArrowBack";
import InfoOutlinedIcon from "@mui/icons-material/InfoOutlined";
import { useTranslation } from "react-i18next";
import { daySeparator, formatDelta, formatLogTime, formatLogTimestamp } from "../../utils/diagnosticsTime";
import { callIdTag, parseStatus, splitSipMessage } from "../../utils/sipMessage";
import { log as applog } from "../../utils/log";
import type { DiagnosticLog } from "./types";

interface Props {
  log: DiagnosticLog;
  /** The previous message's time, for the gap and the day separator. */
  previousTimestamp: number | null;
  expanded: boolean;
  onToggle: () => void;
  /** Narrow the list to this message's dialog. */
  onPickCall: (callId: string) => void;
  /** Whether this message's dialog is the one being shown. */
  pickedCall: boolean;
}

/**
 * One line of the trace: when (to the millisecond, and how long after the
 * previous message), which way, which dialog, and what. Expanded, the
 * headers and the body, with a copy button.
 */
export function MessageRow({ log, previousTimestamp, expanded, onToggle, onPickCall, pickedCall }: Props) {
  const theme = useTheme();
  const { t, i18n } = useTranslation();
  const [copied, setCopied] = useState(false);

  const status = parseStatus(log.summary);
  const isApp = log.direction === "app";
  const isSent = log.direction === "sent";
  const tone =
    status.kind === "response"
      ? status.code >= 400
        ? status.code === 401 || status.code === 407
          ? theme.palette.warning.main
          : theme.palette.error.main
        : status.code >= 200
          ? theme.palette.success.main
          : theme.palette.info.main
      : isApp
        ? theme.palette.secondary.main
        : theme.palette.text.primary;
  const separator = daySeparator(previousTimestamp, log.timestamp, i18n.language);
  const delta = previousTimestamp === null ? null : log.timestamp - previousTimestamp;
  const tag = log.callId ? callIdTag(log.callId) : null;

  const copy = async () => {
    const text = `${formatLogTimestamp(log.timestamp)} ${isApp ? "aria" : isSent ? "->" : "<-"} ${log.remoteAddr}\n${log.raw}`;
    try {
      await navigator.clipboard.writeText(text);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1200);
    } catch (e) {
      applog.error("[diagnostics] copy failed:", e);
    }
  };

  const parsed = expanded && !isApp ? splitSipMessage(log.raw) : null;

  return (
    <>
      {separator && (
        <Box sx={{ display: "flex", alignItems: "center", gap: 1, px: 1, py: 0.5, color: "text.secondary" }}>
          <Box sx={{ flex: 1, borderTop: `1px solid ${alpha(theme.palette.divider, 0.4)}` }} />
          <Typography variant="caption" sx={{ fontSize: "0.65rem", fontWeight: 600 }}>
            {separator}
          </Typography>
          <Box sx={{ flex: 1, borderTop: `1px solid ${alpha(theme.palette.divider, 0.4)}` }} />
        </Box>
      )}
      <Box
        onClick={onToggle}
        onKeyDown={(e) => {
          if (e.key === "Enter" || e.key === " ") {
            e.preventDefault();
            onToggle();
          }
        }}
        role="button"
        tabIndex={0}
        aria-expanded={expanded}
        aria-label={log.summary}
        sx={{
          px: 1,
          py: 0.5,
          mb: 0.25,
          borderRadius: "8px",
          cursor: "pointer",
          contentVisibility: "auto",
          borderLeft: `2px solid ${
            isApp
              ? alpha(theme.palette.secondary.main, 0.6)
              : isSent
                ? alpha(theme.palette.info.main, 0.5)
                : alpha(theme.palette.success.main, 0.5)
          }`,
          bgcolor: expanded ? alpha(theme.palette.primary.main, 0.06) : "transparent",
          "&:hover": { bgcolor: alpha(theme.palette.text.primary, 0.03) },
        }}
      >
        <Box sx={{ display: "flex", alignItems: "center", gap: 0.75, fontFamily: "monospace" }}>
          <Tooltip title={formatLogTimestamp(log.timestamp)} enterDelay={600}>
            <Typography variant="caption" sx={{ fontFamily: "inherit", fontSize: "0.68rem", color: "text.primary", fontVariantNumeric: "tabular-nums" }}>
              {formatLogTime(log.timestamp, i18n.language)}
            </Typography>
          </Tooltip>
          <Typography
            variant="caption"
            sx={{ fontFamily: "inherit", fontSize: "0.62rem", color: "text.secondary", minWidth: 58, fontVariantNumeric: "tabular-nums", opacity: 0.8 }}
          >
            {formatDelta(delta)}
          </Typography>
          {isApp ? (
            <InfoOutlinedIcon sx={{ fontSize: 13, color: "secondary.main" }} titleAccess={t("diagnostics.appEvent")} />
          ) : isSent ? (
            <ArrowForwardIcon sx={{ fontSize: 13, color: "info.main" }} titleAccess={t("diagnostics.tx")} />
          ) : (
            <ArrowBackIcon sx={{ fontSize: 13, color: "success.main" }} titleAccess={t("diagnostics.rx")} />
          )}
          {tag && (
            <Tooltip title={pickedCall ? t("diagnostics.clearCallFilter") : t("diagnostics.filterByCall")} enterDelay={600}>
              <Chip
                label={tag.text}
                size="small"
                onClick={(e) => {
                  e.stopPropagation();
                  onPickCall(log.callId ?? "");
                }}
                sx={{
                  height: 16,
                  fontSize: "0.58rem",
                  fontFamily: "monospace",
                  fontWeight: 700,
                  bgcolor: alpha(`hsl(${tag.hue} 70% 50%)`, pickedCall ? 0.35 : 0.14),
                  color: `hsl(${tag.hue} 60% ${theme.palette.mode === "dark" ? 75 : 30}%)`,
                  "& .MuiChip-label": { px: 0.6 },
                }}
              />
            </Tooltip>
          )}
          <Typography
            variant="body2"
            sx={{
              fontFamily: "inherit",
              fontSize: "0.72rem",
              fontWeight: status.kind === "request" || isApp ? 600 : 500,
              color: tone,
              whiteSpace: "nowrap",
              overflow: "hidden",
              textOverflow: "ellipsis",
              flex: 1,
              minWidth: 0,
            }}
          >
            {log.summary}
          </Typography>
          {!isApp && (
            <Typography variant="caption" sx={{ fontFamily: "inherit", fontSize: "0.6rem", color: "text.secondary", opacity: 0.6, flexShrink: 0 }}>
              {log.remoteAddr}
            </Typography>
          )}
        </Box>
        {expanded && (
          <Box
            onClick={(e) => e.stopPropagation()}
            sx={{
              mt: 0.75,
              p: 1,
              borderRadius: "6px",
              bgcolor: alpha(theme.palette.text.primary, 0.03),
              fontFamily: "monospace",
              fontSize: "0.68rem",
              position: "relative",
              cursor: "text",
              userSelect: "text",
            }}
          >
            <Tooltip title={copied ? t("diagnostics.copiedMessage") : t("diagnostics.copyMessage")}>
              <IconButton size="small" onClick={() => void copy()} sx={{ position: "absolute", top: 4, right: 4 }}>
                {copied ? <CheckIcon sx={{ fontSize: 14 }} /> : <ContentCopyIcon sx={{ fontSize: 14 }} />}
              </IconButton>
            </Tooltip>
            {parsed ? (
              <>
                <Box sx={{ color: tone, fontWeight: 600, mb: 0.5, pr: 4, wordBreak: "break-all" }}>{parsed.startLine}</Box>
                <Box component="table" sx={{ borderCollapse: "collapse", width: "100%" }}>
                  <tbody>
                    {parsed.headers.map(([name, value], i) => (
                      <tr key={`${name}-${i}`}>
                        <Box component="td" sx={{ color: "text.secondary", pr: 1, verticalAlign: "top", whiteSpace: "nowrap" }}>
                          {name}:
                        </Box>
                        <Box component="td" sx={{ wordBreak: "break-all" }}>{value}</Box>
                      </tr>
                    ))}
                  </tbody>
                </Box>
                {parsed.body && (
                  <Box component="pre" sx={{ m: 0, mt: 0.75, pt: 0.75, borderTop: `1px solid ${alpha(theme.palette.divider, 0.3)}`, whiteSpace: "pre-wrap", wordBreak: "break-all", fontFamily: "inherit" }}>
                    {parsed.body}
                  </Box>
                )}
              </>
            ) : (
              <Box component="pre" sx={{ m: 0, pr: 4, whiteSpace: "pre-wrap", wordBreak: "break-word", fontFamily: "inherit" }}>
                {log.raw}
              </Box>
            )}
          </Box>
        )}
      </Box>
    </>
  );
}
