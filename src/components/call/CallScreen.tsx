import { Box, Typography, Avatar, alpha, useTheme, Tooltip } from "@mui/material";
import LockIcon from "@mui/icons-material/Lock";
import LockOpenIcon from "@mui/icons-material/LockOpen";
import PersonIcon from "@mui/icons-material/Person";
import { motion } from "framer-motion";
import { useTranslation } from "react-i18next";
import { useAppStore } from "../../stores/appStore";
import { CallControls } from "./CallControls";
import { AudioLevelMeter } from "./AudioLevelMeter";
import { RecordingIndicator } from "./RecordingIndicator";
import { useCallTimer } from "../../hooks/useCallTimer";
import type { SrtpMode } from "../../types/sip";
import { callerInitials } from "../../utils/callerAvatar";

const stateMessageKeys: Record<string, string> = {
  dialing: "call.calling",
  ringing: "call.ringing",
  incoming: "call.incoming",
  connected: "",
  held: "call.onHold",
  ended: "call.ended",
};

/**
 * Sizes and spacing for the call screen. The sizes are CSS variables on its
 * root so the avatar and the controls shrink together as the window gets
 * shorter. The thresholds are the window's height, which includes the status
 * bar above the call screen. Kept in one object: a second "@media" key in the
 * same sx would silently replace these.
 */
const callScreenLayout = {
  "--call-avatar": "88px",
  "--call-avatar-font": "2rem",
  "--call-control": "52px",
  "--call-hangup": "64px",
  gap: 3,
  pt: 2,
  pb: 2.5,
  "@media (max-height: 680px)": {
    "--call-avatar": "64px",
    "--call-avatar-font": "1.5rem",
    "--call-control": "46px",
    "--call-hangup": "56px",
    gap: 1.5,
    pt: 1.5,
    pb: 2,
  },
  "@media (max-height: 540px)": {
    "--call-avatar": "48px",
    "--call-avatar-font": "1.15rem",
    "--call-control": "40px",
    "--call-hangup": "52px",
    gap: 1,
    pt: 1,
    pb: 1.5,
  },
} as const;

function EncryptionIndicator({
  srtpMode,
  srtpActive,
}: {
  srtpMode: SrtpMode;
  srtpActive: boolean | undefined;
}) {
  const { t } = useTranslation();
  const theme = useTheme();

  // What was negotiated, not what was configured: the backend reports
  // srtpActive with "connected". SRTP is preferred, not required, so a call
  // can be perfectly fine and unencrypted; the pill just has to say so. A
  // call the backend has not reported on yet (undefined) shows nothing
  // rather than guessing.
  if (srtpActive === undefined) return null;

  const isEncrypted = srtpActive;
  const label = isEncrypted ? t("call.encrypted") : t("call.notEncrypted");
  // Only SDES encrypts; a stored "dtls" account was migrated to disabled.
  const tooltip = isEncrypted
    ? t("call.encryptedTooltip")
    : srtpMode === "sdes"
      ? t("call.notEncryptedRefused")
      : t("call.notEncryptedOff");
  const palette = isEncrypted ? theme.palette.success : theme.palette.warning;
  // The main shades are pale enough to wash out on the light background.
  const color = theme.palette.mode === "dark" ? palette.main : palette.dark;
  const Icon = isEncrypted ? LockIcon : LockOpenIcon;

  return (
    <Tooltip title={tooltip} placement="bottom">
      <Box
        tabIndex={0}
        aria-label={`${label}. ${tooltip}`}
        sx={{
          display: "inline-flex",
          alignItems: "center",
          gap: 0.5,
          px: 1.2,
          py: 0.3,
          borderRadius: "12px",
          bgcolor: alpha(palette.main, 0.1),
          border: "1px solid",
          borderColor: alpha(palette.main, 0.25),
          cursor: "default",
          transition: "all 0.2s ease",
          flexShrink: 0,
        }}
      >
        <Icon sx={{ fontSize: 13, color }} />
        <Typography
          variant="caption"
          sx={{
            fontSize: "0.68rem",
            fontWeight: 600,
            letterSpacing: "0.02em",
            color,
            lineHeight: 1.2,
          }}
        >
          {label}
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
  const initials = callerInitials(activeCall.remoteName);

  // `held` is the source of truth for hold; the "held" state exists but the
  // backend never emits it, so a held call stays state "connected" with
  // held = true. Treat it as connected-but-on-hold.
  const isConnected = activeCall.state === "connected" || activeCall.state === "held";
  const isRinging =
    activeCall.state === "dialing" || activeCall.state === "ringing";
  const isOnHold = activeCall.held;
  const accountSrtpMode: SrtpMode =
    accounts.find((a) => a.id === activeCall.accountId)?.srtpMode ?? "disabled";

  // A flex column that fits the space it is given, centred as one group so
  // a tall window does not open a gap between the caller and the controls.
  // When space runs out the caller details give way first (min-height 0,
  // clipped); the meter and the controls never shrink, so hang-up stays on
  // screen. This used to be a space-between column with fixed sizes and 64px
  // of padding whose content was taller than a 720pt window: the overflow
  // went into the view's scroll area, where a hidden macOS scrollbar left
  // hang-up cut off.
  return (
    <Box
      sx={{
        ...callScreenLayout,
        height: "100%",
        minHeight: 0,
        boxSizing: "border-box",
        display: "flex",
        flexDirection: "column",
        alignItems: "center",
        justifyContent: "center",
        px: 3,
        overflow: "hidden",
        background: `radial-gradient(ellipse at 50% 0%, ${alpha(theme.palette.primary.main, 0.1)} 0%, transparent 70%)`,
      }}
    >
      <Box
        sx={{
          flex: "0 1 auto",
          minHeight: 0,
          width: "100%",
          overflow: "hidden",
          display: "flex",
          flexDirection: "column",
          alignItems: "center",
          justifyContent: "center",
          gap: 1,
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
          style={{ borderRadius: "50%", flexShrink: 0 }}
        >
          <Avatar
            sx={{
              width: "var(--call-avatar)",
              height: "var(--call-avatar)",
              bgcolor: alpha(theme.palette.primary.main, 0.12),
              color: "primary.main",
              fontSize: "var(--call-avatar-font)",
              fontWeight: 400,
              fontFamily: '"Google Sans", sans-serif',
              mb: 0.5,
            }}
          >
            {initials ?? (
              <PersonIcon
                aria-hidden
                sx={{ fontSize: "calc(var(--call-avatar) * 0.55)" }}
              />
            )}
          </Avatar>
        </motion.div>

        <Typography
          variant="h5"
          noWrap
          title={cleaned}
          sx={{
            maxWidth: "100%",
            flexShrink: 0,
            fontWeight: 400,
            color: "text.primary",
            textAlign: "center",
            letterSpacing: "-0.01em",
            "@media (max-height: 680px)": { fontSize: "1.25rem" },
          }}
        >
          {cleaned}
        </Typography>

        <Typography
          variant="body2"
          sx={{
            flexShrink: 0,
            color: isConnected ? "primary.main" : "text.secondary",
            fontVariantNumeric: "tabular-nums",
            fontWeight: isConnected ? 500 : 400,
          }}
        >
          {isOnHold
            ? t("call.onHold")
            : isConnected
              ? elapsed
              : stateMessageKeys[activeCall.state]
                ? t(stateMessageKeys[activeCall.state])
                : ""}
        </Typography>

        {isConnected && (
          <EncryptionIndicator
            srtpMode={accountSrtpMode}
            srtpActive={activeCall.srtpActive}
          />
        )}

        {activeCall.recording && (
          <Box sx={{ mt: 0.5, flexShrink: 0 }}>
            <RecordingIndicator variant="full" />
          </Box>
        )}
      </Box>

      {isConnected && (
        <Box sx={{ width: "100%", maxWidth: 280, px: 1, flexShrink: 0 }}>
          <AudioLevelMeter compact />
        </Box>
      )}

      <Box sx={{ width: "100%", maxWidth: 360, flexShrink: 0 }}>
        <CallControls />
      </Box>
    </Box>
  );
}
