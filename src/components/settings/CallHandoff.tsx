import { useState } from "react";
import { Box, Collapse, ListItemButton, ListItemIcon, ListItemText, Switch, Typography } from "@mui/material";
import DevicesIcon from "@mui/icons-material/Devices";
import ExpandLessIcon from "@mui/icons-material/ExpandLess";
import ExpandMoreIcon from "@mui/icons-material/ExpandMore";
import { useTranslation } from "react-i18next";
import { useCallMoveStore } from "../../stores/callMoveStore";
import { useMoveMailbox } from "../../hooks/useCallMove";
import type { HandoffSettings } from "../../utils/callMove";

interface RowProps {
  id: string;
  label: string;
  hint: string;
  checked: boolean;
  disabled?: boolean;
  onChange: (on: boolean) => void;
}

function Row({ id, label, hint, checked, disabled, onChange }: RowProps) {
  return (
    <Box sx={{ display: "flex", alignItems: "center", gap: 1.5, py: 0.75 }}>
      <Box sx={{ flex: 1, minWidth: 0 }}>
        <Typography id={`${id}-label`} variant="body2">
          {label}
        </Typography>
        <Typography id={`${id}-hint`} variant="caption" sx={{ color: "text.secondary", display: "block" }}>
          {hint}
        </Typography>
      </Box>
      <Switch
        size="small"
        checked={checked}
        disabled={disabled}
        onChange={(e) => onChange(e.target.checked)}
        slotProps={{ input: { "aria-labelledby": `${id}-label`, "aria-describedby": `${id}-hint` } }}
      />
    </Box>
  );
}

/** Settings for moving calls between this user's devices (`/me/call-handoff`). */
export function CallHandoff() {
  const { t } = useTranslation();
  const mailbox = useMoveMailbox();
  const [open, setOpen] = useState(false);
  const handoff = useCallMoveStore((s) => s.handoff);
  const loaded = useCallMoveStore((s) => s.handoffLoaded);
  const save = useCallMoveStore((s) => s.saveHandoff);

  if (!mailbox) return null;
  const set = (patch: Partial<HandoffSettings>) => void save(mailbox, { ...handoff, ...patch });
  const disabled = !loaded;

  return (
    <>
      <ListItemButton
        onClick={() => setOpen(!open)}
        aria-expanded={open}
        sx={{ borderRadius: "16px", mb: 0.5 }}
      >
        <ListItemIcon sx={{ minWidth: 40 }}>
          <DevicesIcon sx={{ fontSize: 20 }} />
        </ListItemIcon>
        <ListItemText
          primary={t("callMove.settingsTitle")}
          secondary={t("callMove.settingsIntro")}
          slotProps={{ primary: { sx: { fontSize: "0.9rem" } }, secondary: { sx: { fontSize: "0.7rem" } } }}
        />
        {open ? <ExpandLessIcon sx={{ fontSize: 20 }} /> : <ExpandMoreIcon sx={{ fontSize: 20 }} />}
      </ListItemButton>
      <Collapse in={open} mountOnEnter unmountOnExit>
        <Box sx={{ px: 2, pb: 2 }}>
          <Row
            id="handoff-nearby"
            label={t("callMove.suggestNearby")}
            hint={t("callMove.suggestNearbyHint")}
            checked={handoff.suggestNearby}
            disabled={disabled}
            onChange={(on) => set({ suggestNearby: on })}
          />
          <Row
            id="handoff-lock"
            label={t("callMove.autoMoveOnLock")}
            hint={t("callMove.autoMoveOnLockHint")}
            checked={handoff.autoMoveOnLock}
            disabled={disabled}
            onChange={(on) => set({ autoMoveOnLock: on })}
          />
          {/* Bluetooth proximity needs a native BLE layer the desktop app does
              not have yet; the switch is shown, off and unavailable, rather
              than saved and silently doing nothing. */}
          <Row
            id="handoff-ble"
            label={t("callMove.ble")}
            hint={t("callMove.bleUnavailable")}
            checked={false}
            disabled
            onChange={() => {}}
          />
        </Box>
      </Collapse>
    </>
  );
}
