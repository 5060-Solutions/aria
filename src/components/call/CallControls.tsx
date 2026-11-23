import { useState, useEffect } from "react";
import {
  Box, IconButton, alpha, useTheme, Typography, Dialog, DialogTitle,
  DialogContent, TextField, Button, Popover, List, ListItemButton,
  ListItemIcon, ListItemText, Divider,
} from "@mui/material";
import MicIcon from "@mui/icons-material/Mic";
import MicOffIcon from "@mui/icons-material/MicOff";
import PauseIcon from "@mui/icons-material/Pause";
import PlayArrowIcon from "@mui/icons-material/PlayArrow";
import CallEndIcon from "@mui/icons-material/CallEnd";
import DialpadIcon from "@mui/icons-material/Dialpad";
import HeadsetMicIcon from "@mui/icons-material/HeadsetMic";
import FiberManualRecordIcon from "@mui/icons-material/FiberManualRecord";
import StopIcon from "@mui/icons-material/Stop";
import AddIcCallIcon from "@mui/icons-material/AddIcCall";
import SwapCallsIcon from "@mui/icons-material/SwapCalls";
import CheckIcon from "@mui/icons-material/Check";
import VolumeUpIcon from "@mui/icons-material/VolumeUp";
import { motion } from "framer-motion";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { useAppStore } from "../../stores/appStore";
import CallIcon from "@mui/icons-material/Call";
import { hangupCall, placeCall, sipAnswer, sipMute, sipHold, sipStartRecording, sipStopRecording, sipConferenceEnd, sipSwapCalls, sipSendDtmf, sipTransferBlind } from "../../hooks/useSip";
import PhoneForwardedIcon from "@mui/icons-material/PhoneForwarded";
import { dialUri } from "../../utils/sipUri";
import { DialerButton } from "../dialer/DialerButton";
import { log } from "../../utils/log";
import { errorText, notifyError } from "../../stores/toastStore";
import { pushAudioDevices } from "../../utils/audioDevices";

interface AudioDevice {
  name: string;
  isDefault: boolean;
}

interface AudioDevices {
  inputDevices: AudioDevice[];
  outputDevices: AudioDevice[];
}

interface ControlButtonProps {
  icon: React.ReactNode;
  label: string;
  active?: boolean;
  onClick: (e: React.MouseEvent<HTMLButtonElement>) => void;
}

function ControlButton({ icon, label, active, onClick }: ControlButtonProps) {
  const theme = useTheme();
  return (
    <Box
      sx={{
        display: "flex",
        flexDirection: "column",
        alignItems: "center",
        gap: 0.5,
        minWidth: 0,
      }}
    >
      <motion.div whileTap={{ scale: 0.88 }}>
        <IconButton
          onClick={onClick}
          aria-label={label}
          aria-pressed={active}
          sx={{
            // Set by CallScreen, smaller in a short window.
            width: "var(--call-control, 52px)",
            height: "var(--call-control, 52px)",
            borderRadius: "50%",
            bgcolor: active
              ? "primary.main"
              : alpha(theme.palette.text.primary, 0.06),
            color: active ? "primary.contrastText" : "text.primary",
            transition: "all 0.15s ease",
            "&:hover": {
              bgcolor: active
                ? "primary.dark"
                : alpha(theme.palette.text.primary, 0.1),
            },
          }}
        >
          {icon}
        </IconButton>
      </motion.div>
      <Typography
        variant="caption"
        noWrap
        title={label}
        sx={{
          fontSize: "0.65rem",
          lineHeight: 1.3,
          color: "text.secondary",
          maxWidth: "100%",
        }}
      >
        {label}
      </Typography>
    </Box>
  );
}

export function CallControls() {
  const { t } = useTranslation();
  const activeCall = useAppStore((s) => s.activeCall);
  const activeCalls = useAppStore((s) => s.activeCalls);
  const conferences = useAppStore((s) => s.conferences);
  const toggleMute = useAppStore((s) => s.toggleMute);
  const toggleHold = useAppStore((s) => s.toggleHold);
  const setCallRecording = useAppStore((s) => s.setCallRecording);
  const updateCall = useAppStore((s) => s.updateCall);
  const setPrimaryCall = useAppStore((s) => s.setPrimaryCall);
  const endConference = useAppStore((s) => s.endConference);
  const accounts = useAppStore((s) => s.accounts);

  const selectedInputDevice = useAppStore((s) => s.selectedInputDevice);
  const selectedOutputDevice = useAppStore((s) => s.selectedOutputDevice);
  const setSelectedInputDevice = useAppStore((s) => s.setSelectedInputDevice);
  const setSelectedOutputDevice = useAppStore((s) => s.setSelectedOutputDevice);

  const [addCallDialogOpen, setAddCallDialogOpen] = useState(false);
  const [newCallUri, setNewCallUri] = useState("");
  const [transferDialogOpen, setTransferDialogOpen] = useState(false);
  const [transferUri, setTransferUri] = useState("");
  const [transferring, setTransferring] = useState(false);
  const [dtmfAnchor, setDtmfAnchor] = useState<HTMLButtonElement | null>(null);
  const [audioAnchor, setAudioAnchor] = useState<HTMLButtonElement | null>(null);
  const [audioDevices, setAudioDevices] = useState<AudioDevices | null>(null);

  // Load audio devices when popover opens
  useEffect(() => {
    if (audioAnchor) {
      invoke<AudioDevices>("get_audio_devices")
        .then(setAudioDevices)
        .catch(() => {});
    }
  }, [audioAnchor]);

  if (!activeCall) return null;

  const isIncoming = activeCall.state === "incoming";

  const handleAnswer = async () => {
    try {
      await sipAnswer(activeCall.id);
    } catch (e) {
      log.error("Answer failed:", e);
    }
  };

  const handleDecline = async () => {
    await hangupCall(activeCall);
  };

  if (isIncoming) {
    return (
      <Box sx={{ display: "flex", justifyContent: "center", gap: 6 }}>
        <Box sx={{ display: "flex", flexDirection: "column", alignItems: "center", gap: 0.5 }}>
          <motion.div whileTap={{ scale: 0.88 }}>
            <IconButton
              onClick={handleDecline}
              aria-label={t("call.decline")}
              sx={{
                width: "var(--call-hangup, 64px)",
                height: "var(--call-hangup, 64px)",
                borderRadius: "50%",
                bgcolor: "error.main",
                color: "#fff",
                boxShadow: (theme) =>
                  `0 6px 24px ${alpha(theme.palette.error.main, 0.4)}`,
                "&:hover": { bgcolor: "error.dark" },
              }}
            >
              <CallEndIcon sx={{ fontSize: 28 }} />
            </IconButton>
          </motion.div>
          <Typography variant="caption" sx={{ fontSize: "0.7rem", color: "text.secondary" }}>
            {t("call.decline")}
          </Typography>
        </Box>
        <Box sx={{ display: "flex", flexDirection: "column", alignItems: "center", gap: 0.5 }}>
          <motion.div whileTap={{ scale: 0.88 }}>
            <IconButton
              onClick={handleAnswer}
              aria-label={t("call.answer")}
              sx={{
                width: "var(--call-hangup, 64px)",
                height: "var(--call-hangup, 64px)",
                borderRadius: "50%",
                bgcolor: "success.main",
                color: "#fff",
                boxShadow: (theme) =>
                  `0 6px 24px ${alpha(theme.palette.success.main, 0.4)}`,
                "&:hover": { bgcolor: "success.dark" },
              }}
            >
              <CallIcon sx={{ fontSize: 28 }} />
            </IconButton>
          </motion.div>
          <Typography variant="caption" sx={{ fontSize: "0.7rem", color: "text.secondary" }}>
            {t("call.answer")}
          </Typography>
        </Box>
      </Box>
    );
  }

  const connectedCalls = activeCalls.filter(
    (c) => c.state === "connected" || c.state === "held" || c.state === "conferenced"
  );
  const heldCalls = activeCalls.filter((c) => c.state === "held");
  const hasMultipleCalls = connectedCalls.length > 1;
  const isInConference = activeCall.conferenceId !== undefined;
  const canAddCall = activeCall.state === "connected" && !isInConference;
  // No Merge: the engine has no mixer, so "merged" calls could hear Aria's
  // user but not each other. It comes back with mixing (or a PBX bridge).

  // Stopping the recording and filing history both happen when the backend
  // reports the call ended, so hangup is the same whichever side ends it.
  const handleHangup = async () => {
    // In a conference, End hangs up every leg. It used to hang up only the
    // primary one, leaving the other leg live and still marked "conferenced".
    if (isInConference && activeCall.conferenceId) {
      const legs = activeCalls.filter((c) => c.conferenceId === activeCall.conferenceId);
      await sipConferenceEnd(activeCall.conferenceId).catch(() => {});
      endConference(activeCall.conferenceId);
      await Promise.all(legs.map((c) => hangupCall(c).catch(() => {})));
      return;
    }
    await hangupCall(activeCall);
  };

  // Mute and hold flip the UI at once and put it back if the backend could
  // not do it. Failures used to be ignored, so the screen could say "Held"
  // while the far end still heard everything.
  const handleMute = async () => {
    const { id, muted } = activeCall;
    toggleMute();
    try {
      await sipMute(id, !muted);
    } catch (e) {
      log.error("Mute toggle failed:", e);
      updateCall(id, { muted });
      notifyError(t(muted ? "errors.unmute" : "errors.mute", { error: errorText(e) }));
    }
  };

  const handleHold = async () => {
    const { id, held, state } = activeCall;
    toggleHold();
    try {
      await sipHold(id, !held);
    } catch (e) {
      log.error("Hold toggle failed:", e);
      updateCall(id, { held, state });
      notifyError(t(held ? "errors.resume" : "errors.hold", { error: errorText(e) }));
    }
  };

  const handleRecord = async () => {
    try {
      if (activeCall.recording) {
        await sipStopRecording(activeCall.id);
        setCallRecording(activeCall.id, false);
      } else {
        const path = await sipStartRecording(activeCall.id);
        setCallRecording(activeCall.id, true, path);
      }
    } catch (e) {
      log.error("Recording toggle failed:", e);
      notifyError(t("errors.record", { error: errorText(e) }));
    }
  };

  const handleAddCall = async () => {
    if (!newCallUri.trim()) return;

    try {
      await sipHold(activeCall.id, true).catch((e) => {
        notifyError(t("errors.hold", { error: errorText(e) }));
        throw e;
      });
      updateCall(activeCall.id, { held: true, state: "held" });
      const account = accounts.find((a) => a.id === activeCall.accountId);
      await placeCall(
        {
          uri: dialUri(newCallUri, account, useAppStore.getState().defaultCountry),
          accountId: activeCall.accountId,
        },
        { asAdditionalCall: true },
      );
      setAddCallDialogOpen(false);
      setNewCallUri("");
    } catch (e) {
      log.error("Add call failed:", e);
    }
  };

  const handleTransfer = async () => {
    if (!transferUri.trim()) return;
    setTransferring(true);
    const account = accounts.find((a) => a.id === activeCall.accountId);
    try {
      await sipTransferBlind(
        activeCall.id,
        dialUri(transferUri, account, useAppStore.getState().defaultCountry),
      );
      // The 202 only means the far end will try. The backend ends the call
      // when the transfer's NOTIFY reports success (sipfrag 200); a refusal
      // arrives later as a transfer-progress event, which useSipEvents shows.
      setTransferDialogOpen(false);
      setTransferUri("");
    } catch (e) {
      log.error("Transfer failed:", e);
      notifyError(t("errors.transferFailed", { error: errorText(e) }));
    } finally {
      setTransferring(false);
    }
  };

  const handleSwap = async () => {
    const otherCall = heldCalls.find((c) => c.id !== activeCall.id);
    if (!otherCall) return;

    try {
      await sipSwapCalls(activeCall.id, otherCall.id);
      updateCall(activeCall.id, { held: true, state: "held" });
      updateCall(otherCall.id, { held: false, state: "connected" });
      setPrimaryCall(otherCall.id);
    } catch (e) {
      log.error("Swap failed:", e);
      notifyError(t("errors.swap", { error: errorText(e) }));
    }
  };

  const handleSelectDevice = (type: "input" | "output", name: string) => {
    if (type === "input") {
      setSelectedInputDevice(name);
    } else {
      setSelectedOutputDevice(name);
    }
    // Push to backend. The call in progress keeps its devices (the engine
    // cannot move a running stream); the menu says so.
    pushAudioDevices(
      invoke,
      type === "input" ? name : selectedInputDevice,
      type === "output" ? name : selectedOutputDevice,
    ).catch(() => {});
  };

  const hasNonDefaultDevice = audioDevices && (
    audioDevices.inputDevices.length > 1 || audioDevices.outputDevices.length > 1
  );

  const controls: (ControlButtonProps & { key: string })[] = [
    {
      key: "mute",
      icon: activeCall.muted ? <MicOffIcon /> : <MicIcon />,
      label: activeCall.muted ? t("call.unmute") : t("call.mute"),
      active: activeCall.muted,
      onClick: handleMute,
    },
    {
      key: "keypad",
      icon: <DialpadIcon />,
      label: t("call.keypad"),
      active: Boolean(dtmfAnchor),
      onClick: (e) => setDtmfAnchor(dtmfAnchor ? null : e.currentTarget),
    },
    {
      key: "audio",
      icon: <HeadsetMicIcon />,
      label: t("call.audio"),
      onClick: (e) => setAudioAnchor(e.currentTarget),
    },
    {
      key: "hold",
      icon: activeCall.held ? <PlayArrowIcon /> : <PauseIcon />,
      label: activeCall.held ? t("call.resume") : t("call.hold"),
      active: activeCall.held,
      onClick: handleHold,
    },
    {
      key: "record",
      icon: activeCall.recording ? <StopIcon /> : <FiberManualRecordIcon />,
      label: activeCall.recording ? t("call.stopRec") : t("call.record"),
      active: activeCall.recording,
      onClick: handleRecord,
    },
  ];
  if (canAddCall) {
    controls.push(
      {
        key: "add",
        icon: <AddIcCallIcon />,
        label: t("call.addCall"),
        onClick: () => setAddCallDialogOpen(true),
      },
      {
        key: "transfer",
        icon: <PhoneForwardedIcon />,
        label: t("call.transfer"),
        onClick: () => setTransferDialogOpen(true),
      },
    );
  }
  if (hasMultipleCalls && heldCalls.length > 0) {
    controls.push({
      key: "swap",
      icon: <SwapCallsIcon />,
      label: t("call.swap"),
      onClick: handleSwap,
    });
  }
  // As few rows as fit, each as full as possible: four across for seven or
  // eight buttons, three for five or six. A short last row is centred rather
  // than left hanging under the first column.
  const columns = controls.length <= 4 ? controls.length : controls.length <= 6 ? 3 : 4;

  return (
    <>
      <Box
        sx={{
          display: "flex",
          flexDirection: "column",
          gap: 2.5,
          "@media (max-height: 680px)": { gap: 1.5 },
        }}
      >
        <Box
          sx={{
            display: "flex",
            flexWrap: "wrap",
            justifyContent: "center",
            rowGap: 1.5,
            "@media (max-height: 680px)": { rowGap: 1 },
          }}
        >
          {controls.map(({ key, ...button }) => (
            <Box key={key} sx={{ width: `${100 / columns}%`, minWidth: 0, px: 0.25 }}>
              <ControlButton {...button} />
            </Box>
          ))}
        </Box>

        {/* Conference indicator */}
        {isInConference && (
          <Box sx={{ textAlign: "center" }}>
            <Typography variant="caption" color="primary">
              {t("call.inConference")} ({conferences.find(c => c.id === activeCall.conferenceId)?.callIds.length ?? 0} {t("call.participants")})
            </Typography>
          </Box>
        )}

        <Box sx={{ display: "flex", justifyContent: "center" }}>
          <motion.div whileTap={{ scale: 0.88 }}>
            <IconButton
              onClick={handleHangup}
              aria-label={t("call.hangUp")}
              sx={{
                width: "var(--call-hangup, 64px)",
                height: "var(--call-hangup, 64px)",
                borderRadius: "50%",
                bgcolor: "error.main",
                color: "#fff",
                boxShadow: (theme) =>
                  `0 6px 24px ${alpha(theme.palette.error.main, 0.4)}`,
                "&:hover": { bgcolor: "error.dark" },
              }}
            >
              <CallEndIcon sx={{ fontSize: 28 }} />
            </IconButton>
          </motion.div>
        </Box>
      </Box>

      {/* DTMF keypad popover */}
      <Popover
        open={Boolean(dtmfAnchor)}
        anchorEl={dtmfAnchor}
        onClose={() => setDtmfAnchor(null)}
        anchorOrigin={{ vertical: "top", horizontal: "center" }}
        transformOrigin={{ vertical: "bottom", horizontal: "center" }}
        slotProps={{
          paper: {
            sx: {
              borderRadius: "16px",
              p: 2,
              mb: 1,
            },
          },
        }}
      >
        <Box
          sx={{
            display: "grid",
            gridTemplateColumns: "repeat(3, 1fr)",
            gap: 1,
            justifyItems: "center",
          }}
        >
          {[
            { digit: "1", letters: "" },
            { digit: "2", letters: "ABC" },
            { digit: "3", letters: "DEF" },
            { digit: "4", letters: "GHI" },
            { digit: "5", letters: "JKL" },
            { digit: "6", letters: "MNO" },
            { digit: "7", letters: "PQRS" },
            { digit: "8", letters: "TUV" },
            { digit: "9", letters: "WXYZ" },
            { digit: "*", letters: "" },
            { digit: "0", letters: "+" },
            { digit: "#", letters: "" },
          ].map(({ digit, letters }) => (
            <DialerButton
              key={digit}
              digit={digit}
              letters={letters || undefined}
              onPress={(d) => {
                sipSendDtmf(activeCall.id, d).catch(() => {});
              }}
            />
          ))}
        </Box>
      </Popover>

      {/* Audio device picker popover */}
      <Popover
        open={Boolean(audioAnchor)}
        anchorEl={audioAnchor}
        onClose={() => setAudioAnchor(null)}
        anchorOrigin={{ vertical: "top", horizontal: "center" }}
        transformOrigin={{ vertical: "bottom", horizontal: "center" }}
        slotProps={{
          paper: {
            sx: {
              borderRadius: "16px",
              minWidth: 240,
              maxWidth: 300,
              mb: 1,
            },
          },
        }}
      >
        {audioDevices && (
          <Box sx={{ py: 1 }}>
            <Typography
              variant="caption"
              sx={{ px: 2, pb: 0.5, display: "block", color: "text.secondary" }}
            >
              {t("call.devicesNextCall")}
            </Typography>
            {/* Microphone section */}
            <Typography
              variant="overline"
              sx={{
                px: 2,
                py: 0.5,
                display: "block",
                fontSize: "0.6rem",
                color: "text.secondary",
                letterSpacing: 1,
              }}
            >
              {t("settings.microphone")}
            </Typography>
            <List dense disablePadding>
              {audioDevices.inputDevices.map((device) => {
                const isSelected = selectedInputDevice === device.name
                  || (!selectedInputDevice && device.isDefault);
                return (
                  <ListItemButton
                    key={device.name}
                    selected={isSelected}
                    onClick={() => handleSelectDevice("input", device.name)}
                    sx={{
                      py: 0.5,
                      px: 2,
                      borderRadius: "8px",
                      mx: 0.5,
                      minHeight: 36,
                    }}
                  >
                    <ListItemIcon sx={{ minWidth: 28 }}>
                      <MicIcon sx={{ fontSize: 16, color: isSelected ? "primary.main" : "text.secondary" }} />
                    </ListItemIcon>
                    <ListItemText
                      primary={device.name}
                      slotProps={{
                        primary: {
                          noWrap: true,
                          sx: {
                            fontSize: "0.78rem",
                            fontWeight: isSelected ? 600 : 400,
                          },
                        },
                      }}
                    />
                    {isSelected && <CheckIcon sx={{ fontSize: 16, color: "primary.main" }} />}
                  </ListItemButton>
                );
              })}
            </List>

            {hasNonDefaultDevice && <Divider sx={{ my: 0.5 }} />}

            {/* Speaker section */}
            <Typography
              variant="overline"
              sx={{
                px: 2,
                py: 0.5,
                display: "block",
                fontSize: "0.6rem",
                color: "text.secondary",
                letterSpacing: 1,
              }}
            >
              {t("settings.speaker")}
            </Typography>
            <List dense disablePadding>
              {audioDevices.outputDevices.map((device) => {
                const isSelected = selectedOutputDevice === device.name
                  || (!selectedOutputDevice && device.isDefault);
                return (
                  <ListItemButton
                    key={device.name}
                    selected={isSelected}
                    onClick={() => handleSelectDevice("output", device.name)}
                    sx={{
                      py: 0.5,
                      px: 2,
                      borderRadius: "8px",
                      mx: 0.5,
                      minHeight: 36,
                    }}
                  >
                    <ListItemIcon sx={{ minWidth: 28 }}>
                      <VolumeUpIcon sx={{ fontSize: 16, color: isSelected ? "primary.main" : "text.secondary" }} />
                    </ListItemIcon>
                    <ListItemText
                      primary={device.name}
                      slotProps={{
                        primary: {
                          noWrap: true,
                          sx: {
                            fontSize: "0.78rem",
                            fontWeight: isSelected ? 600 : 400,
                          },
                        },
                      }}
                    />
                    {isSelected && <CheckIcon sx={{ fontSize: 16, color: "primary.main" }} />}
                  </ListItemButton>
                );
              })}
            </List>

            <Typography
              variant="caption"
              sx={{
                display: "block",
                px: 2,
                pt: 1,
                pb: 0.5,
                fontSize: "0.6rem",
                color: "text.disabled",
              }}
            >
              {t("settings.audioNote")}
            </Typography>
          </Box>
        )}
      </Popover>

      {/* Add Call Dialog */}
      <Dialog
        open={addCallDialogOpen}
        onClose={() => setAddCallDialogOpen(false)}
        maxWidth="xs"
        fullWidth
      >
        <DialogTitle>{t("call.addCall", "Add Call")}</DialogTitle>
        <DialogContent>
          <TextField
            autoFocus
            margin="dense"
            label={t("call.phoneNumber")}
            fullWidth
            variant="outlined"
            value={newCallUri}
            onChange={(e) => setNewCallUri(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                handleAddCall();
              }
            }}
          />
          <Box sx={{ display: "flex", gap: 1, mt: 2, justifyContent: "flex-end" }}>
            <Button onClick={() => setAddCallDialogOpen(false)}>
              {t("common.cancel", "Cancel")}
            </Button>
            <Button variant="contained" onClick={handleAddCall} disabled={!newCallUri.trim()}>
              {t("call.dial")}
            </Button>
          </Box>
        </DialogContent>
      </Dialog>

      {/* Blind transfer dialog */}
      <Dialog
        open={transferDialogOpen}
        onClose={() => setTransferDialogOpen(false)}
        maxWidth="xs"
        fullWidth
      >
        <DialogTitle>{t("call.transfer", "Transfer")}</DialogTitle>
        <DialogContent>
          <Typography variant="caption" sx={{ color: "text.secondary", display: "block", mb: 1 }}>
            {t("call.transferHint")}
          </Typography>
          <TextField
            autoFocus
            margin="dense"
            label={t("call.phoneNumber")}
            fullWidth
            variant="outlined"
            value={transferUri}
            onChange={(e) => setTransferUri(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") handleTransfer();
            }}
          />
          <Box sx={{ display: "flex", gap: 1, mt: 2, justifyContent: "flex-end" }}>
            <Button onClick={() => setTransferDialogOpen(false)}>
              {t("common.cancel", "Cancel")}
            </Button>
            <Button
              variant="contained"
              onClick={handleTransfer}
              disabled={!transferUri.trim() || transferring}
            >
              {t("call.transfer", "Transfer")}
            </Button>
          </Box>
        </DialogContent>
      </Dialog>
    </>
  );
}
