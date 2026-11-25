/** The ladder view: every message as an arrow between Aria and a remote address. */
import { useEffect, useState, useRef } from "react";
import {
  Box,
  Typography,
  alpha,
  useTheme,
} from "@mui/material";
import { useTranslation } from "react-i18next";
import { formatLogTime } from "../../utils/diagnosticsTime";
import type { DiagnosticLog } from "./types";

export function SipLadderDiagram({ logs }: { logs: DiagnosticLog[] }) {
  const theme = useTheme();
  const { t, i18n } = useTranslation();
  const scrollRef = useRef<HTMLDivElement>(null);
  const [expandedIdx, setExpandedIdx] = useState<number | null>(null);

  useEffect(() => {
    const el = scrollRef.current;
    if (el) {
      el.scrollTo({ top: el.scrollHeight, behavior: "smooth" });
    }
  }, [logs.length]);


  if (logs.length === 0) {
    return (
      <Box sx={{ textAlign: "center", py: 4, color: "text.secondary" }}>
        <Typography variant="caption" sx={{ opacity: 0.5, fontSize: "0.7rem" }}>
          {t("diagnostics.sipMessagesEmpty")}
        </Typography>
      </Box>
    );
  }

  // Extract unique remote addresses for columns
  const remoteAddrs = Array.from(new Set(logs.map((l) => l.remoteAddr)));
  const colWidth = 140;
  const localLabel = "Aria";
  const totalWidth = colWidth * (remoteAddrs.length + 1) + 60; // +60 for timestamp

  const getMethodColor = (summary: string) => {
    if (summary.match(/\b[4-6]\d\d\b/)) return theme.palette.error.main;
    if (summary.match(/\b20[02]\b/)) return theme.palette.success.main;
    if (summary.match(/\b40[17]\b/)) return theme.palette.warning.main;
    if (summary.match(/\b18[03]\b/)) return theme.palette.info.main;
    return theme.palette.text.primary;
  };

  // Extract short label from SIP summary line
  const getShortLabel = (summary: string) => {
    // Response: "SIP/2.0 200 OK" -> "200 OK"
    const respMatch = summary.match(/^SIP\/2\.0\s+(\d+\s+.*)$/);
    if (respMatch) return respMatch[1];
    // Request: "REGISTER sip:..." -> "REGISTER"
    const reqMatch = summary.match(/^(\w+)\s+/);
    if (reqMatch) return reqMatch[1];
    return summary;
  };

  return (
    <Box ref={scrollRef} sx={{ overflow: "auto", px: 1, pb: 2 }}>
      <Box sx={{ minWidth: totalWidth }}>
        {/* Column headers */}
        <Box
          sx={{
            display: "flex",
            alignItems: "flex-end",
            position: "sticky",
            top: 0,
            bgcolor: "background.default",
            zIndex: 1,
            pb: 0.5,
            borderBottom: `1px solid ${alpha(theme.palette.divider, 0.15)}`,
          }}
        >
          {/* Timestamp column */}
          <Box sx={{ width: 60, flexShrink: 0 }} />
          {/* Local column */}
          <Box sx={{ width: colWidth, textAlign: "center", flexShrink: 0 }}>
            <Typography
              variant="caption"
              sx={{
                fontWeight: 700,
                fontSize: "0.65rem",
                color: theme.palette.info.main,
              }}
            >
              {localLabel}
            </Typography>
          </Box>
          {/* Remote columns */}
          {remoteAddrs.map((addr) => (
            <Box
              key={addr}
              sx={{ width: colWidth, textAlign: "center", flexShrink: 0 }}
            >
              <Typography
                variant="caption"
                sx={{
                  fontWeight: 700,
                  fontSize: "0.6rem",
                  fontFamily: "monospace",
                  color: theme.palette.success.main,
                }}
              >
                {addr}
              </Typography>
            </Box>
          ))}
        </Box>

        {/* Lifelines + Messages */}
        {logs.map((entry, i) => {
          const isSent = entry.direction === "sent";
          const remoteColIdx = remoteAddrs.indexOf(entry.remoteAddr);
          const localCol = 0;
          const remoteCol = remoteColIdx + 1;
          const fromCol = isSent ? localCol : remoteCol;
          const toCol = isSent ? remoteCol : localCol;
          const leftCol = Math.min(fromCol, toCol);
          const rightCol = Math.max(fromCol, toCol);
          const label = getShortLabel(entry.summary);
          const color = getMethodColor(entry.summary);
          const isExpanded = expandedIdx === i;

          return (
            <Box key={`${entry.timestamp}-${i}`}>
              <Box
                onClick={() => setExpandedIdx(isExpanded ? null : i)}
                sx={{
                  display: "flex",
                  alignItems: "center",
                  position: "relative",
                  height: 28,
                  cursor: "pointer",
                  "&:hover": {
                    bgcolor: alpha(theme.palette.text.primary, 0.03),
                  },
                }}
              >
                {/* Timestamp */}
                <Box sx={{ width: 60, flexShrink: 0 }}>
                  <Typography
                    variant="caption"
                    sx={{
                      fontFamily: "monospace",
                      fontSize: "0.5rem",
                      color: "text.secondary",
                      opacity: 0.6,
                    }}
                  >
                    {formatLogTime(entry.timestamp, i18n.language)}
                  </Typography>
                </Box>

                {/* Lifeline columns with vertical lines */}
                <Box
                  sx={{
                    display: "flex",
                    position: "relative",
                    flex: 1,
                    height: "100%",
                  }}
                >
                  {/* Vertical lifelines */}
                  {Array.from({ length: remoteAddrs.length + 1 }).map(
                    (_, colIdx) => (
                      <Box
                        key={colIdx}
                        sx={{
                          position: "absolute",
                          left: colIdx * colWidth + colWidth / 2,
                          top: 0,
                          bottom: 0,
                          width: 1,
                          bgcolor: alpha(theme.palette.divider, 0.15),
                        }}
                      />
                    ),
                  )}

                  {/* Arrow line */}
                  <Box
                    sx={{
                      position: "absolute",
                      left: leftCol * colWidth + colWidth / 2,
                      width: (rightCol - leftCol) * colWidth,
                      top: "50%",
                      height: 1.5,
                      bgcolor: color,
                      opacity: 0.7,
                    }}
                  />

                  {/* Arrow head */}
                  <Box
                    sx={{
                      position: "absolute",
                      left:
                        toCol * colWidth +
                        colWidth / 2 +
                        (isSent ? -6 : 2),
                      top: "50%",
                      transform: "translateY(-50%)",
                      width: 0,
                      height: 0,
                      borderTop: "4px solid transparent",
                      borderBottom: "4px solid transparent",
                      ...(isSent
                        ? { borderLeft: `6px solid ${color}` }
                        : { borderRight: `6px solid ${color}` }),
                      opacity: 0.7,
                    }}
                  />

                  {/* Label on the arrow */}
                  <Box
                    sx={{
                      position: "absolute",
                      left:
                        leftCol * colWidth +
                        colWidth / 2 +
                        ((rightCol - leftCol) * colWidth) / 2,
                      top: 0,
                      transform: "translateX(-50%)",
                    }}
                  >
                    <Typography
                      variant="caption"
                      sx={{
                        fontSize: "0.58rem",
                        fontFamily: "monospace",
                        fontWeight: 600,
                        color,
                        bgcolor: "background.default",
                        px: 0.5,
                        whiteSpace: "nowrap",
                      }}
                    >
                      {label}
                    </Typography>
                  </Box>
                </Box>
              </Box>

              {/* Expanded raw message */}
              {isExpanded && (
                <Box
                  sx={{
                    mx: 1,
                    mb: 0.5,
                    p: 1,
                    borderRadius: "6px",
                    bgcolor: alpha(theme.palette.text.primary, 0.03),
                    maxHeight: 200,
                    overflow: "auto",
                    "&::-webkit-scrollbar": { width: 3 },
                    "&::-webkit-scrollbar-thumb": {
                      bgcolor: alpha(theme.palette.text.primary, 0.1),
                      borderRadius: 2,
                    },
                  }}
                >
                  <Typography
                    component="pre"
                    sx={{
                      fontSize: "0.55rem",
                      fontFamily: "monospace",
                      whiteSpace: "pre-wrap",
                      wordBreak: "break-all",
                      m: 0,
                      color: "text.secondary",
                      lineHeight: 1.4,
                    }}
                  >
                    {entry.raw}
                  </Typography>
                </Box>
              )}
            </Box>
          );
        })}
      </Box>
    </Box>
  );
}

// --- Account Tab Content ---
