import { Box, Typography, Avatar, alpha, useTheme, Tooltip } from "@mui/material";
import LockIcon from "@mui/icons-material/Lock";
import LockOpenIcon from "@mui/icons-material/LockOpen";
import { motion } from "framer-motion";
import { useTranslation } from "react-i18next";
import { useAppStore } from "../../stores/appStore";
import { CallControls } from "./CallControls";
import { AudioLevelMeter } from "./AudioLevelMeter";
import { RecordingIndicator } from "./RecordingIndicator";
import { useCallTimer } from "../../hooks/useCallTimer";
import type { SrtpMode } from "../../types/sip";

const stateMessageKeys: Record<string, string> = {
  dialing: "call.calling",
  ringing: "call.ringing",
  incoming: "call.incoming",
  connected: "",
  held: "call.onHold",
  ended: "call.ended",
};

function EncryptionIndicator({ srtpMode }: { srtpMode: SrtpMode }) {
  const { t } = useTranslation();
  const theme = useTheme();

  const isEncrypted = srtpMode !== "disabled";
  const label = isEncrypted
    ? srtpMode === "dtls"
      ? t("call.encryptionDtls")
      : t("call.encryptionSdes")
    : t("call.notEncrypted");

  return (
    <Tooltip title={label} placement="bottom">
      <Box
        sx={{
          display: "inline-flex",
          alignItems: "center",
          gap: 0.5,
          px: 1.2,
          py: 0.3,
          borderRadius: "12px",
          bgcolor: isEncrypted
            ? alpha(theme.palette.success.main, 0.1)
            : alpha(theme.palette.warning.main, 0.08),
          border: "1px solid",
          borderColor: isEncrypted
            ? alpha(theme.palette.success.main, 0.25)
            : alpha(theme.palette.warning.main, 0.2),
          cursor: "default",
          transition: "all 0.2s ease",
        }}
      >
        {isEncrypted ? (
          <LockIcon
            sx={{
              fontSize: 13,
              color: "success.main",
            }}
          />
        ) : (
          <LockOpenIcon
            sx={{
              fontSize: 13,
              color: "warning.main",
            }}
          />
        )}
        <Typography
          variant="caption"
          sx={{
            fontSize: "0.68rem",
            fontWeight: 600,
            letterSpacing: "0.04em",
            color: isEncrypted ? "success.main" : "warning.main",
            lineHeight: 1,
          }}
        >
          {isEncrypted ? srtpMode.toUpperCase() : "—"}
        </Typography>
      </Box>
    </Tooltip>
  );
}

export function CallScreen() {
  const { t } = useTranslation();
  const activeCall = useAppStore((s) => s.activeCall);
  const accounts = useAppStore((s) => s.accounts);
  const theme = useTheme();
  const elapsed = useCallTimer(activeCall?.connectTime ?? null);

  if (!activeCall) return null;

  const displayName = activeCall.remoteName || activeCall.remoteUri;
  const cleaned = displayName.replace(/^sip:/, "");
  const initials = cleaned.substring(0, 2).toUpperCase();

  const isConnected = activeCall.state === "connected";
  const isRinging =
    activeCall.state === "dialing" || activeCall.state === "ringing";
  const accountSrtpMode: SrtpMode =
    accounts.find((a) => a.id === activeCall.accountId)?.srtpMode ?? "disabled";

  return (
    <Box
      sx={{
        height: "100%",
        display: "flex",
        flexDirection: "column",
        alignItems: "center",
        justifyContent: "space-between",
        py: 5,
        px: 3,
        background: `radial-gradient(ellipse at 50% 0%, ${alpha(theme.palette.primary.main, 0.1)} 0%, transparent 70%)`,
      }}
    >
      <Box
        sx={{
          display: "flex",
          flexDirection: "column",
          alignItems: "center",
          gap: 2,
          mt: 3,
        }}
      >
        <motion.div
          animate={
            isRinging
              ? {
                  scale: [1, 1.06, 1],
                  boxShadow: [
                    `0 0 0 0 ${alpha(theme.palette.primary.main, 0.3)}`,
                    `0 0 0 20px ${alpha(theme.palette.primary.main, 0)}`,
                    `0 0 0 0 ${alpha(theme.palette.primary.main, 0)}`,
                  ],
                }
              : {}
          }
          transition={isRinging ? { duration: 1.5, repeat: Infinity } : {}}
          style={{ borderRadius: "50%" }}
        >
          <Avatar
            sx={{
              width: 100,
              height: 100,
              bgcolor: alpha(theme.palette.primary.main, 0.12),
              color: "primary.main",
              fontSize: "2.2rem",
              fontWeight: 400,
              fontFamily: '"Google Sans", sans-serif',
            }}
          >
            {initials}
          </Avatar>
        </motion.div>

        <Typography
          variant="h5"
          sx={{
            fontWeight: 400,
            color: "text.primary",
            textAlign: "center",
            letterSpacing: "-0.01em",
          }}
        >
          {cleaned}
        </Typography>

        <Typography
          variant="body2"
          sx={{
            color: isConnected ? "primary.main" : "text.secondary",
            fontVariantNumeric: "tabular-nums",
            fontWeight: isConnected ? 500 : 400,
          }}
        >
          {isConnected ? elapsed : (stateMessageKeys[activeCall.state] ? t(stateMessageKeys[activeCall.state]) : "")}
        </Typography>

        {isConnected && (
          <EncryptionIndicator srtpMode={accountSrtpMode} />
        )}

        {activeCall.recording && (
          <Box sx={{ mt: 2 }}>
            <RecordingIndicator variant="full" />
          </Box>
        )}
      </Box>

      {isConnected && (
        <Box sx={{ width: "100%", maxWidth: 280, px: 1 }}>
          <AudioLevelMeter compact />
        </Box>
      )}

      <CallControls />
    </Box>
  );
}
