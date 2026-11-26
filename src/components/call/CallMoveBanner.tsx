import { Box, Button, IconButton, Typography, alpha } from "@mui/material";
import CloseIcon from "@mui/icons-material/Close";
import PhoneForwardedIcon from "@mui/icons-material/PhoneForwarded";
import { useTranslation } from "react-i18next";
import { useAppStore } from "../../stores/appStore";
import { useCallMoveStore } from "../../stores/callMoveStore";
import { useMoveMailbox } from "../../hooks/useCallMove";
import { callsElsewhere, remoteLabel } from "../../utils/callMove";

const bar = {
  display: "flex",
  alignItems: "center",
  gap: 1.5,
  px: 2,
  py: 0.75,
  minHeight: 44,
  borderBottom: "1px solid",
  borderColor: "divider",
} as const;

/**
 * "Call with <remote> on <device> · Move here" while the user has a call on
 * another device, and "Continue on <device>" when the PBX suggests one for
 * the call held here.
 */
export function CallMoveBanner() {
  const { t } = useTranslation();
  const mailbox = useMoveMailbox();
  const calls = useCallMoveStore((s) => s.calls);
  const current = useCallMoveStore((s) => s.current);
  const pending = useCallMoveStore((s) => s.pending);
  const suggestion = useCallMoveStore((s) => s.suggestion);
  const moveHere = useCallMoveStore((s) => s.moveHere);
  const move = useCallMoveStore((s) => s.move);
  const dismiss = useCallMoveStore((s) => s.dismissSuggestion);
  const liveSipIds = useAppStore((s) => s.activeCalls.filter((c) => c.state !== "ended").map((c) => c.sipCallId ?? ""));
  const hasLive = liveSipIds.length > 0;

  if (!mailbox) return null;

  // Continue on <device>: for the call held here, and only that one.
  const suggested = suggestion ? calls.find((c) => c.id === suggestion.callId) : undefined;
  if (suggestion && suggested && liveSipIds.includes(suggested.sipCallId)) {
    const call = suggested;
    const device = suggestion.device.label || t("callMove.anotherDevice");
    return (
      <Box role="status" sx={{ ...bar, bgcolor: (th) => alpha(th.palette.primary.main, 0.08) }}>
        <PhoneForwardedIcon aria-hidden sx={{ fontSize: 18, color: "primary.main" }} />
        <Typography variant="body2" sx={{ flex: 1, minWidth: 0 }} noWrap>
          {t("callMove.continueOn", { device })}
        </Typography>
        <Button
          size="small"
          variant="contained"
          disabled={!call}
          onClick={() => {
            if (call) void move(mailbox, call, suggestion.device.registrationId);
            dismiss();
          }}
        >
          {t("callMove.continue")}
        </Button>
        <IconButton size="small" aria-label={t("callMove.dismiss")} onClick={dismiss}>
          <CloseIcon sx={{ fontSize: 18 }} />
        </IconButton>
      </Box>
    );
  }

  if (hasLive) return null;
  const elsewhere = callsElsewhere(calls, liveSipIds, current);
  const call = elsewhere[0];
  if (!call) return null;
  const who = remoteLabel(call);
  const device = call.device.label || t("callMove.anotherDevice");
  const moving = pending !== null;

  return (
    <Box role="status" sx={{ ...bar, bgcolor: (th) => alpha(th.palette.primary.main, 0.08) }}>
      <PhoneForwardedIcon aria-hidden sx={{ fontSize: 18, color: "primary.main" }} />
      <Typography variant="body2" sx={{ flex: 1, minWidth: 0 }} noWrap>
        {t("callMove.activeElsewhere", { who, device })}
      </Typography>
      <Button
        size="small"
        variant="contained"
        disabled={moving}
        aria-label={t("callMove.moveHereLabel", { who, device })}
        onClick={() => void moveHere(mailbox, call)}
      >
        {moving ? t("callMove.moving") : t("callMove.moveHere")}
      </Button>
    </Box>
  );
}
