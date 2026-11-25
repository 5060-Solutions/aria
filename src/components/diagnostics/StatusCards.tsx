/** The status cards at the top of the Diagnostics window: connection and live calls. */
import { useCallback } from "react";
import {
  Box,
  Typography,
  Chip,
  alpha,
  useTheme,
  Divider,
  Button,
} from "@mui/material";
import FiberManualRecordIcon from "@mui/icons-material/FiberManualRecord";
import SignalCellularAltIcon from "@mui/icons-material/SignalCellularAlt";
import PhoneInTalkIcon from "@mui/icons-material/PhoneInTalk";
import SwapVertIcon from "@mui/icons-material/SwapVert";
import TimerOutlinedIcon from "@mui/icons-material/TimerOutlined";
import { useTranslation } from "react-i18next";
import type { AccountStatus, CallStatusInfo, RtpStats } from "./types";
import { registrationColor } from "./registrationColor";

function StatusDot({ color }: { color: string }) {
  return (
    <FiberManualRecordIcon
      sx={{
        fontSize: 10,
        color,
        filter: `drop-shadow(0 0 3px ${color})`,
      }}
    />
  );
}

function StatRow({
  label,
  value,
  mono,
}: {
  label: string;
  value: string | number | null | undefined;
  mono?: boolean;
}) {
  const theme = useTheme();
  return (
    <Box
      sx={{
        display: "flex",
        justifyContent: "space-between",
        alignItems: "center",
        py: 0.3,
      }}
    >
      <Typography
        variant="caption"
        sx={{ color: "text.secondary", fontSize: "0.7rem" }}
      >
        {label}
      </Typography>
      <Typography
        variant="caption"
        sx={{
          fontFamily: mono ? "monospace" : undefined,
          fontSize: "0.7rem",
          color: theme.palette.text.primary,
          fontWeight: 500,
        }}
      >
        {value ?? "-"}
      </Typography>
    </Box>
  );
}

function SectionCard({
  children,
  sx,
}: {
  children: React.ReactNode;
  sx?: object;
}) {
  const theme = useTheme();
  return (
    <Box
      sx={{
        mx: 1.5,
        mb: 1,
        p: 1.5,
        borderRadius: "12px",
        bgcolor: alpha(theme.palette.background.paper, 0.6),
        border: `1px solid ${alpha(theme.palette.divider, 0.08)}`,
        ...sx,
      }}
    >
      {children}
    </Box>
  );
}

function SectionHeader({
  icon,
  title,
  trailing,
}: {
  icon: React.ReactNode;
  title: string;
  trailing?: React.ReactNode;
}) {
  return (
    <Box
      sx={{
        display: "flex",
        alignItems: "center",
        gap: 0.75,
        mb: 0.75,
      }}
    >
      {icon}
      <Typography
        variant="caption"
        sx={{ fontWeight: 600, fontSize: "0.72rem", flex: 1 }}
      >
        {title}
      </Typography>
      {trailing}
    </Box>
  );
}


function useRegistrationLabel() {
  const { t } = useTranslation();
  return useCallback((state: string): string => {
    switch (state) {
      case "registered":
        return t("diagnostics.registered");
      case "registering":
        return t("diagnostics.registering");
      case "reconnecting":
        return t("diagnostics.reconnecting");
      case "error":
        return t("diagnostics.error");
      default:
        return t("diagnostics.unregistered");
    }
  }, [t]);
}

function formatUptime(secs: number | null | undefined): string {
  if (secs == null) return "-";
  const h = Math.floor(secs / 3600);
  const m = Math.floor((secs % 3600) / 60);
  const s = secs % 60;
  if (h > 0) return `${h}h ${m}m`;
  if (m > 0) return `${m}m ${s}s`;
  return `${s}s`;
}

function formatDuration(secs: number | null | undefined): string {
  if (secs == null) return "0:00";
  const m = Math.floor(secs / 60);
  const s = secs % 60;
  return `${m}:${s.toString().padStart(2, "0")}`;
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

function latencyColor(ms: number | null | undefined): string {
  if (ms == null) return "#9e9e9e";
  if (ms < 100) return "#4caf50";
  if (ms < 250) return "#ff9800";
  return "#f44336";
}

function lossPercent(stats: RtpStats): string {
  const total = stats.packetsReceived + stats.packetsLost;
  if (total === 0) return "0%";
  return `${((stats.packetsLost / total) * 100).toFixed(1)}%`;
}

// --- Connection Card ---


/** What the Connection card's buttons do to an account, by its id. */
export interface AccountActions {
  reregister: (accountId: string) => void;
  unregister: (accountId: string) => void;
  register: (accountId: string) => void;
  /** Which account has an action in flight, if any. */
  busy: string | null;
}

export function ConnectionCard({
  account,
  latencyMs,
  actions,
}: {
  account: AccountStatus;
  latencyMs: number | null;
  actions: AccountActions;
}) {
  const { t } = useTranslation();
  const theme = useTheme();
  const registrationLabel = useRegistrationLabel();
  const state = account.registrationState ?? "unregistered";
  const color = registrationColor(state);
  const busy = actions.busy === account.accountId;
  const up = state === "registered" || state === "registering" || state === "reconnecting";
  return (
    <SectionCard>
      <SectionHeader
        icon={<SignalCellularAltIcon sx={{ fontSize: 14, color }} />}
        title={t("diagnostics.connection")}
        trailing={
          <Box sx={{ display: "flex", alignItems: "center", gap: 0.5 }}>
            <StatusDot color={color} />
            <Typography variant="caption" sx={{ fontSize: "0.65rem", color, fontWeight: 600 }}>
              {registrationLabel(state)}
            </Typography>
          </Box>
        }
      />
      {account.registrationError && (
        <Typography
          variant="caption"
          sx={{
            color: "error.main",
            fontSize: "0.65rem",
            display: "block",
            mb: 0.5,
            px: 0.5,
            py: 0.25,
            borderRadius: "6px",
            bgcolor: alpha(theme.palette.error.main, 0.08),
          }}
        >
          {account.registrationError}
        </Typography>
      )}
      <StatRow
        label={t("diagnostics.server")}
        value={
          account.domain
            ? `${account.domain}${account.serverAddress ? ` (${account.serverAddress})` : ""}`
            : account.serverAddress
        }
        mono
      />
      <StatRow label={t("diagnostics.transport")} value={account.transportType?.toUpperCase()} />
      <StatRow label={t("diagnostics.local")} value={account.localAddress} mono />
      <StatRow label={t("diagnostics.publicNat")} value={account.publicAddress} mono />
      <StatRow
        label={t("diagnostics.identity")}
        value={account.username && account.domain ? `${account.username}@${account.domain}` : undefined}
        mono
      />
      <Divider sx={{ my: 0.75, opacity: 0.3 }} />
      <Box sx={{ display: "flex", justifyContent: "space-between", alignItems: "center", gap: 1 }}>
        <Box sx={{ display: "flex", alignItems: "center", gap: 1.5, minWidth: 0 }}>
          <Box sx={{ display: "flex", alignItems: "center", gap: 0.5 }}>
            <TimerOutlinedIcon sx={{ fontSize: 12, color: "text.secondary" }} />
            <Typography variant="caption" sx={{ fontSize: "0.65rem", color: "text.secondary" }}>
              {t("diagnostics.uptime")}: {formatUptime(account.uptimeSecs)}
            </Typography>
          </Box>
          <Box sx={{ display: "flex", alignItems: "center", gap: 0.5 }}>
            <SwapVertIcon sx={{ fontSize: 12, color: latencyColor(latencyMs) }} />
            <Typography
              variant="caption"
              sx={{ fontSize: "0.65rem", fontFamily: "monospace", color: latencyColor(latencyMs), fontWeight: 600 }}
            >
              {latencyMs != null ? `${latencyMs.toFixed(0)}ms` : "-"}
            </Typography>
          </Box>
        </Box>
        <Box sx={{ display: "flex", gap: 0.5, flexShrink: 0 }}>
          {up ? (
            <>
              <Button
                size="small"
                variant="outlined"
                disabled={busy}
                onClick={() => actions.reregister(account.accountId)}
                sx={{ fontSize: "0.65rem", py: 0, px: 1, minWidth: 0, borderRadius: "8px", textTransform: "none" }}
              >
                {t("connection.reregister")}
              </Button>
              <Button
                size="small"
                variant="text"
                color="inherit"
                disabled={busy}
                onClick={() => actions.unregister(account.accountId)}
                sx={{ fontSize: "0.65rem", py: 0, px: 1, minWidth: 0, borderRadius: "8px", textTransform: "none", color: "text.secondary" }}
              >
                {t("connection.unregister")}
              </Button>
            </>
          ) : (
            <Button
              size="small"
              variant="outlined"
              disabled={busy}
              onClick={() => actions.register(account.accountId)}
              sx={{ fontSize: "0.65rem", py: 0, px: 1, minWidth: 0, borderRadius: "8px", textTransform: "none" }}
            >
              {state === "error" ? t("connection.retryNow") : t("connection.register")}
            </Button>
          )}
        </Box>
      </Box>
    </SectionCard>
  );
}

export function ActiveCallCard({ call }: { call: CallStatusInfo }) {
  const { t } = useTranslation();
  const theme = useTheme();
  const stats = call.rtpStats;
  const hasStats = stats != null;

  return (
    <SectionCard>
      <SectionHeader
        icon={
          <PhoneInTalkIcon sx={{ fontSize: 14, color: "success.main" }} />
        }
        title={t("diagnostics.activeCall")}
        trailing={
          <Chip
            label={call.state}
            size="small"
            sx={{
              height: 18,
              fontSize: "0.6rem",
              fontWeight: 700,
              bgcolor: alpha(theme.palette.success.main, 0.12),
              color: "success.main",
            }}
          />
        }
      />

      <StatRow label={t("diagnostics.remote")} value={call.remoteUri} mono />
      <StatRow
        label={t("diagnostics.direction")}
        value={call.direction === "inbound" ? t("diagnostics.inbound") : t("diagnostics.outbound")}
      />
      <StatRow label={t("diagnostics.duration")} value={formatDuration(call.durationSecs)} />
      <StatRow label={t("diagnostics.codec")} value={call.codec ?? "-"} />

      {hasStats && (
        <>
          <Divider sx={{ my: 0.75, opacity: 0.3 }} />
          <Box
            sx={{
              display: "grid",
              gridTemplateColumns: "1fr 1fr",
              gap: 0.3,
            }}
          >
            <StatRow
              label={t("diagnostics.txPackets")}
              value={stats.packetsSent.toLocaleString()}
              mono
            />
            <StatRow
              label={t("diagnostics.rxPackets")}
              value={stats.packetsReceived.toLocaleString()}
              mono
            />
            <StatRow label={t("diagnostics.txBytes")} value={formatBytes(stats.bytesSent)} mono />
            <StatRow
              label={t("diagnostics.rxBytes")}
              value={formatBytes(stats.bytesReceived)}
              mono
            />
          </Box>
          <Divider sx={{ my: 0.5, opacity: 0.2 }} />
          <Box
            sx={{
              display: "flex",
              justifyContent: "space-around",
              mt: 0.25,
            }}
          >
            <Box sx={{ textAlign: "center" }}>
              <Typography
                variant="caption"
                sx={{
                  fontSize: "0.9rem",
                  fontWeight: 700,
                  fontFamily: "monospace",
                  color:
                    stats.packetsLost > 0
                      ? "warning.main"
                      : "text.primary",
                }}
              >
                {lossPercent(stats)}
              </Typography>
              <Typography
                variant="caption"
                sx={{
                  fontSize: "0.55rem",
                  color: "text.secondary",
                  display: "block",
                }}
              >
                {t("diagnostics.packetLoss")}
              </Typography>
            </Box>
            <Box sx={{ textAlign: "center" }}>
              <Typography
                variant="caption"
                sx={{
                  fontSize: "0.9rem",
                  fontWeight: 700,
                  fontFamily: "monospace",
                  color:
                    stats.jitterMs > 30
                      ? "warning.main"
                      : "text.primary",
                }}
              >
                {stats.jitterMs.toFixed(1)}
              </Typography>
              <Typography
                variant="caption"
                sx={{
                  fontSize: "0.55rem",
                  color: "text.secondary",
                  display: "block",
                }}
              >
                {t("diagnostics.jitterMs")}
              </Typography>
            </Box>
          </Box>
        </>
      )}
    </SectionCard>
  );
}

// --- SIP Log Entry ---
