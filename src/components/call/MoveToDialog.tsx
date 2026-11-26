import { useEffect, useState } from "react";
import { Box, Button, CircularProgress, Dialog, DialogContent, DialogTitle, List, ListItemButton, ListItemText, Typography } from "@mui/material";
import { useTranslation } from "react-i18next";
import { useAppStore } from "../../stores/appStore";
import { useCallMoveStore } from "../../stores/callMoveStore";
import { useMoveMailbox } from "../../hooks/useCallMove";
import { otherOnlineDevices } from "../../utils/callMove";
import type { Mailbox } from "../../utils/voicemail";

/** "Move to...": the user's other online devices, for the call on screen. */
export function MoveToDialog({ open, onClose }: { open: boolean; onClose: () => void }) {
  const { t } = useTranslation();
  const mailbox = useMoveMailbox();
  return (
    <Dialog open={open} onClose={onClose} maxWidth="xs" fullWidth aria-labelledby="move-to-title">
      <DialogTitle id="move-to-title">{t("callMove.moveTo")}</DialogTitle>
      <DialogContent>
        {/* Mounted afresh each time the dialog opens, so every opening starts
            by reading the devices again. */}
        {open && <Devices mailbox={mailbox} onClose={onClose} />}
        <Box sx={{ display: "flex", justifyContent: "flex-end", mt: 1 }}>
          <Button onClick={onClose}>{t("common.cancel", "Cancel")}</Button>
        </Box>
      </DialogContent>
    </Dialog>
  );
}

function Devices({ mailbox, onClose }: { mailbox: Mailbox | null; onClose: () => void }) {
  const { t } = useTranslation();
  const sipCallId = useAppStore((s) => s.activeCall?.sipCallId);
  const devices = useCallMoveStore((s) => s.devices);
  const calls = useCallMoveStore((s) => s.calls);
  const current = useCallMoveStore((s) => s.current);
  const move = useCallMoveStore((s) => s.move);
  // Loading until this device's own id and the list are both known, so the
  // list never offers this device itself; failed when the PBX could not be
  // read, so stale devices are not offered either.
  const [status, setStatus] = useState<"loading" | "ready" | "failed">("loading");

  useEffect(() => {
    if (!mailbox) return;
    let cancelled = false;
    const store = useCallMoveStore.getState();
    void Promise.all([store.ensureCurrent(mailbox, true), store.loadDevices(mailbox), store.refreshCalls(mailbox)]).then(
      ([own, listed]) => {
        if (!cancelled) setStatus(own && listed ? "ready" : "failed");
      },
    );
    return () => {
      cancelled = true;
    };
  }, [mailbox]);

  if (status === "loading") {
    return (
      <Box sx={{ display: "flex", justifyContent: "center", py: 2 }}>
        <CircularProgress size={22} />
      </Box>
    );
  }
  if (status === "failed") {
    return (
      <Typography variant="body2" sx={{ color: "text.secondary" }}>
        {t("callMove.devicesUnavailable")}
      </Typography>
    );
  }
  const others = otherOnlineDevices(devices, current);
  // The PBX's id for the call held here: the active call whose leg is ours.
  const pbxCall = sipCallId ? calls.find((c) => c.sipCallId === sipCallId) : undefined;
  if (others.length === 0) {
    return (
      <Typography variant="body2" sx={{ color: "text.secondary" }}>
        {t("callMove.noOtherDevices")}
      </Typography>
    );
  }
  return (
    <List dense aria-label={t("callMove.devicesList")}>
      {others.map((d) => (
        <ListItemButton
          key={d.registrationId}
          disabled={!mailbox || !pbxCall}
          onClick={async () => {
            if (!mailbox || !pbxCall) return;
            onClose();
            await move(mailbox, pbxCall, d.registrationId);
          }}
          sx={{ borderRadius: "12px" }}
        >
          <ListItemText primary={d.label || t("callMove.anotherDevice")} />
        </ListItemButton>
      ))}
    </List>
  );
}
