import { useEffect } from "react";
import { Box, Button, Dialog, DialogContent, DialogTitle, List, ListItemButton, ListItemText, Typography } from "@mui/material";
import { useTranslation } from "react-i18next";
import { useAppStore } from "../../stores/appStore";
import { useCallMoveStore } from "../../stores/callMoveStore";
import { useMoveMailbox } from "../../hooks/useCallMove";
import { otherOnlineDevices } from "../../utils/callMove";

/** "Move to...": the user's other online devices, for the call on screen. */
export function MoveToDialog({ open, onClose }: { open: boolean; onClose: () => void }) {
  const { t } = useTranslation();
  const mailbox = useMoveMailbox();
  const sipCallId = useAppStore((s) => s.activeCall?.sipCallId);
  const devices = useCallMoveStore((s) => s.devices);
  const calls = useCallMoveStore((s) => s.calls);
  const current = useCallMoveStore((s) => s.current);
  const move = useCallMoveStore((s) => s.move);

  useEffect(() => {
    if (!open || !mailbox) return;
    const store = useCallMoveStore.getState();
    void store.ensureCurrent(mailbox);
    void store.loadDevices(mailbox);
    void store.refreshCalls(mailbox);
  }, [open, mailbox]);

  const others = otherOnlineDevices(devices, current);
  // The PBX's id for the call held here: the active call whose leg is ours.
  const pbxCall = sipCallId ? calls.find((c) => c.sipCallId === sipCallId) : undefined;

  return (
    <Dialog open={open} onClose={onClose} maxWidth="xs" fullWidth aria-labelledby="move-to-title">
      <DialogTitle id="move-to-title">{t("callMove.moveTo")}</DialogTitle>
      <DialogContent>
        {others.length === 0 ? (
          <Typography variant="body2" sx={{ color: "text.secondary" }}>
            {t("callMove.noOtherDevices")}
          </Typography>
        ) : (
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
        )}
        <Box sx={{ display: "flex", justifyContent: "flex-end", mt: 1 }}>
          <Button onClick={onClose}>{t("common.cancel", "Cancel")}</Button>
        </Box>
      </DialogContent>
    </Dialog>
  );
}
