import { useEffect, useState } from "react";
import {
  Box,
  Button,
  Chip,
  CircularProgress,
  Popover,
  Typography,
  alpha,
  useTheme,
} from "@mui/material";
import CheckIcon from "@mui/icons-material/Check";
import CloseIcon from "@mui/icons-material/Close";
import CircleIcon from "@mui/icons-material/Circle";
import RadioButtonUncheckedIcon from "@mui/icons-material/RadioButtonUnchecked";
import RemoveIcon from "@mui/icons-material/Remove";
import ChevronRightIcon from "@mui/icons-material/ChevronRight";
import { invoke } from "@tauri-apps/api/core";
import { getVersion } from "@tauri-apps/api/app";
import { useTranslation } from "react-i18next";
import { useAppStore } from "../../stores/appStore";
import { retryNow } from "../../hooks/useSip";
import { stateColors, stateLabelsKeys } from "./statusChip";
import {
  accountLine,
  copyDetails,
  labelValues,
  registeredLine,
  retryLine,
  stepLabelKey,
} from "../../utils/connectionReport";
import { hintKey } from "../../utils/connectionHints";
import { log } from "../../utils/log";
import type { ConnectionReport, ConnectionStep } from "../../types/connection";

const ICON_SIZE = 16;

function StepIcon({ status }: { status: ConnectionStep["status"] }) {
  switch (status) {
    case "ok":
      return <CheckIcon sx={{ fontSize: ICON_SIZE, color: "#4caf50" }} />;
    case "failed":
      return <CloseIcon sx={{ fontSize: ICON_SIZE, color: "#f44336" }} />;
    case "running":
      return <CircularProgress size={14} sx={{ color: "#ff9800" }} />;
    case "skipped":
      return <RemoveIcon sx={{ fontSize: ICON_SIZE, color: "text.disabled" }} />;
    case "pending":
      return <RadioButtonUncheckedIcon sx={{ fontSize: ICON_SIZE, color: "text.disabled" }} />;
  }
}

function osName(): string {
  const ua = navigator.userAgent;
  if (/Mac OS X/.test(ua)) return "macOS";
  if (/Windows/.test(ua)) return "Windows";
  if (/Linux/.test(ua)) return "Linux";
  return navigator.platform || "unknown";
}

interface Props {
  anchorEl: HTMLElement | null;
  open: boolean;
  onClose: () => void;
  accountId: string;
}

/**
 * What is behind the status chip: registration as a list of steps, the exact
 * failure under the one that broke, a hint for it, and a way to try again.
 */
export function ConnectionPanel({ anchorEl, open, onClose, accountId }: Props) {
  const { t } = useTranslation();
  const theme = useTheme();
  const report = useAppStore((s) => s.connectionReports[accountId]);
  const plan = useAppStore((s) => s.retryPlans[accountId]) ?? { kind: "none" as const };
  const registrationState = useAppStore(
    (s) => s.accountStates[accountId]?.registrationState ?? "unregistered",
  );
  const setConnectionReport = useAppStore((s) => s.setConnectionReport);
  const [now, setNow] = useState(() => Date.now());
  const [copied, setCopied] = useState(false);

  // The backend may have reported before this window listened; ask once.
  useEffect(() => {
    if (!open || report) return;
    invoke<ConnectionReport | null>("get_connection_report", { accountId })
      .then((r) => {
        if (r) setConnectionReport(r);
      })
      .catch((e) => log.error("[ConnectionPanel] get_connection_report failed:", e));
  }, [open, report, accountId, setConnectionReport]);

  // A ticking clock only while the panel shows a countdown or an expiry.
  const ticking = open && (plan.kind === "scheduled" || report?.outcome === "registered");
  useEffect(() => {
    if (!ticking) return;
    const id = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(id);
  }, [ticking]);

  // Gated on the account's state, not on the report: the backend answers a
  // second click with "already_registering", and a report left mid-step by
  // a failure elsewhere must never lock the button.
  const busy = registrationState === "registering";
  const color = stateColors[registrationState];

  const handleRetry = () => {
    void retryNow(accountId).catch((e) => log.error("[ConnectionPanel] retry failed:", e));
  };

  const handleCopy = async () => {
    if (!report) return;
    const appVersion = await getVersion().catch(() => "unknown");
    const text = copyDetails(report, plan, { appVersion, os: osName(), now: Date.now() }, t);
    try {
      await navigator.clipboard.writeText(text);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1500);
    } catch (e) {
      log.error("[ConnectionPanel] clipboard write failed:", e);
    }
  };

  const hint = report ? hintKey(report) : null;
  const footerLine = report
    ? (retryLine(report, plan, now, t, registrationState) ?? registeredLine(report, now, t))
    : null;

  return (
    <Popover
      open={open}
      anchorEl={anchorEl}
      onClose={onClose}
      anchorOrigin={{ vertical: "bottom", horizontal: "right" }}
      transformOrigin={{ vertical: "top", horizontal: "right" }}
      slotProps={{ paper: { sx: { borderRadius: "12px", width: 360, p: 2, mt: 0.5 } } }}
    >
      <Box sx={{ display: "flex", alignItems: "center", justifyContent: "space-between", mb: 0.5 }}>
        <Typography variant="subtitle2" sx={{ fontWeight: 600 }}>
          {t("connection.title")}
        </Typography>
        <Chip
          icon={<CircleIcon sx={{ fontSize: 8, color: `${color} !important` }} />}
          label={t(stateLabelsKeys[registrationState])}
          size="small"
          variant="outlined"
          sx={{ height: 24, fontSize: "0.7rem", borderColor: alpha(color, 0.3), color: "text.secondary" }}
        />
      </Box>

      {report ? (
        <>
          <Typography variant="caption" sx={{ color: "text.secondary", display: "block", mb: 1 }}>
            {accountLine(report)}
          </Typography>

          {report.steps.map((step) => {
            const failed = step.status === "failed";
            return (
              <Box
                key={step.kind}
                sx={{
                  display: "flex",
                  gap: 1.25,
                  alignItems: "flex-start",
                  py: 0.75,
                  borderTop: `1px solid ${alpha(theme.palette.divider, 0.12)}`,
                  opacity: step.status === "pending" ? 0.5 : 1,
                }}
              >
                <Box sx={{ width: 18, display: "flex", justifyContent: "center", pt: "2px", flex: "none" }}>
                  <StepIcon status={step.status} />
                </Box>
                <Box sx={{ minWidth: 0 }}>
                  <Typography variant="body2">{t(stepLabelKey(step.kind), labelValues(report))}</Typography>
                  {step.detail && (
                    <Typography
                      variant="caption"
                      sx={{
                        display: "block",
                        color: failed ? "#f0a0a0" : "text.secondary",
                        fontFamily: failed ? "ui-monospace, Menlo, monospace" : undefined,
                        userSelect: "text",
                        whiteSpace: "pre-wrap",
                        wordBreak: "break-word",
                      }}
                    >
                      {step.detail}
                    </Typography>
                  )}
                </Box>
              </Box>
            );
          })}

          {hint && (
            <Box
              sx={{
                mt: 1.25,
                p: 1.25,
                borderRadius: "10px",
                bgcolor: alpha(theme.palette.text.primary, 0.05),
              }}
            >
              <Typography variant="caption" sx={{ display: "block" }}>
                <strong>{t("connection.hintTitle")}.</strong> {t(hint)}
              </Typography>
            </Box>
          )}
        </>
      ) : (
        <Typography variant="caption" sx={{ color: "text.secondary", display: "block", mb: 1 }}>
          {t(stateLabelsKeys[registrationState])}
        </Typography>
      )}

      <Box sx={{ display: "flex", gap: 1, alignItems: "center", mt: 1.5 }}>
        <Button
          variant="contained"
          size="small"
          onClick={handleRetry}
          disabled={busy}
          sx={{ borderRadius: "10px", whiteSpace: "nowrap" }}
        >
          {report?.outcome === "registered" ? t("connection.reregister") : t("connection.retryNow")}
        </Button>
        <Button
          variant="outlined"
          size="small"
          onClick={() => void handleCopy()}
          disabled={!report}
          sx={{ borderRadius: "10px", whiteSpace: "nowrap" }}
        >
          {copied ? t("connection.copied") : t("connection.copy")}
        </Button>
      </Box>

      <Box sx={{ display: "flex", gap: 1, alignItems: "center", justifyContent: "space-between", mt: 0.75 }}>
        <Typography variant="caption" sx={{ color: "text.secondary", minWidth: 0 }}>
          {footerLine ?? ""}
        </Typography>
        <Button
          size="small"
          endIcon={<ChevronRightIcon sx={{ fontSize: 16 }} />}
          onClick={() => invoke("open_debug_window").catch(() => {})}
          sx={{ color: "text.secondary", whiteSpace: "nowrap", flex: "none", px: 0.5, minWidth: 0 }}
        >
          {t("connection.diagnostics")}
        </Button>
      </Box>
    </Popover>
  );
}
