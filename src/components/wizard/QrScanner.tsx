import { useState, useEffect, useRef, useCallback } from "react";
import {
  Box,
  Button,
  TextField,
  Typography,
  IconButton,
  alpha,
  useTheme,
  CircularProgress,
} from "@mui/material";
import CloseIcon from "@mui/icons-material/Close";
import QrCodeScannerIcon from "@mui/icons-material/QrCodeScanner";
import ContentPasteIcon from "@mui/icons-material/ContentPaste";
import CameraAltIcon from "@mui/icons-material/CameraAlt";
import CheckCircleOutlineIcon from "@mui/icons-material/CheckCircleOutline";
import { Html5Qrcode } from "html5-qrcode";
import { useTranslation } from "react-i18next";
import { log } from "../../utils/log";

export interface QrProvisionData {
  server: string;
  port: number;
  username: string;
  password: string;
  displayName: string;
  transport: string;
  voicemail: string;
}

/**
 * Parse an aria://provision?... URI into structured provisioning data.
 */
function parseAriaUri(uri: string): QrProvisionData | null {
  try {
    // Handle both aria://provision?... and direct JSON payloads
    if (uri.startsWith("{")) {
      const json = JSON.parse(uri);
      if (json.type === "aria-sip") {
        return {
          server: json.server ?? "",
          port: json.port ?? 5060,
          username: json.username ?? "",
          password: json.password ?? "",
          displayName: json.display_name ?? json.username ?? "",
          transport: json.transport ?? "udp",
          voicemail: json.voicemail ?? "*97",
        };
      }
      return null;
    }

    if (!uri.startsWith("aria://provision")) return null;

    // Parse as URL - aria://provision?key=value&...
    // URL constructor needs a proper scheme, so we replace aria:// with http://
    const url = new URL(uri.replace("aria://", "http://dummy/"));
    const params = url.searchParams;

    const server = params.get("server");
    const user = params.get("user");
    const pass = params.get("pass");

    if (!server || !user || !pass) return null;

    // Password is base64-encoded in the URI
    let password: string;
    try {
      password = atob(pass);
    } catch {
      password = pass; // If not valid base64, use as-is
    }

    return {
      server,
      port: parseInt(params.get("port") ?? "5060", 10),
      username: user,
      password,
      displayName: params.get("name") ?? user,
      transport: params.get("transport") ?? "udp",
      voicemail: params.get("vm") ?? "*97",
    };
  } catch (e) {
    log.error("[QrScanner] Failed to parse URI:", e);
    return null;
  }
}

interface QrScannerProps {
  onProvisioned: (data: QrProvisionData) => void;
  onCancel: () => void;
}

export function QrScanner({ onProvisioned, onCancel }: QrScannerProps) {
  const { t } = useTranslation();
  const theme = useTheme();
  const [mode, setMode] = useState<"choose" | "camera" | "paste">("choose");
  const [pasteValue, setPasteValue] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [cameraActive, setCameraActive] = useState(false);
  const scannerRef = useRef<Html5Qrcode | null>(null);
  const [containerId] = useState(() => "qr-reader-" + Date.now());
  const hasProcessedRef = useRef(false);

  // Cleanup camera on unmount
  useEffect(() => {
    return () => {
      if (scannerRef.current) {
        scannerRef.current.stop().catch(() => {});
        scannerRef.current.clear();
        scannerRef.current = null;
      }
    };
  }, []);

  const handleScanSuccess = useCallback(
    (decodedText: string) => {
      if (hasProcessedRef.current) return;
      const data = parseAriaUri(decodedText);
      if (data) {
        hasProcessedRef.current = true;
        // Stop camera before callback
        if (scannerRef.current) {
          scannerRef.current.stop().catch(() => {});
        }
        onProvisioned(data);
      }
    },
    [onProvisioned]
  );

  const startCamera = useCallback(async () => {
    setError(null);
    setCameraActive(true);
    setMode("camera");

    // Wait for the container to render
    await new Promise((resolve) => setTimeout(resolve, 100));

    try {
      const scanner = new Html5Qrcode(containerId);
      scannerRef.current = scanner;

      await scanner.start(
        { facingMode: "environment" },
        {
          fps: 10,
          qrbox: { width: 250, height: 250 },
          aspectRatio: 1,
        },
        handleScanSuccess,
        () => {
          // Ignore scan failures (no QR detected in frame)
        }
      );
    } catch (e) {
      log.error("[QrScanner] Camera error:", e);
      setCameraActive(false);
      setError(
        "Could not access camera. Make sure camera permissions are granted."
      );
      setMode("choose");
    }
  }, [handleScanSuccess, containerId]);

  const stopCamera = useCallback(async () => {
    if (scannerRef.current) {
      try {
        await scannerRef.current.stop();
        scannerRef.current.clear();
      } catch {
        // ignore
      }
      scannerRef.current = null;
    }
    setCameraActive(false);
  }, []);

  const handlePaste = useCallback(() => {
    const trimmed = pasteValue.trim();
    if (!trimmed) {
      setError("Please paste an aria:// provisioning link.");
      return;
    }

    const data = parseAriaUri(trimmed);
    if (!data) {
      setError(
        "Invalid provisioning link. Expected an aria://provision?... URL or JSON payload."
      );
      return;
    }

    onProvisioned(data);
  }, [pasteValue, onProvisioned]);

  const handlePasteFromClipboard = useCallback(async () => {
    try {
      const text = await navigator.clipboard.readText();
      setPasteValue(text);
      // Auto-detect and provision if valid
      const data = parseAriaUri(text.trim());
      if (data) {
        onProvisioned(data);
      }
    } catch {
      setError("Could not read clipboard. Please paste manually.");
    }
  }, [onProvisioned]);

  return (
    <Box
      sx={{
        position: "fixed",
        inset: 0,
        bgcolor: "background.default",
        zIndex: 1300,
        display: "flex",
        flexDirection: "column",
      }}
    >
      {/* Header */}
      <Box
        sx={{
          display: "flex",
          alignItems: "center",
          justifyContent: "space-between",
          px: 2,
          py: 1.5,
          borderBottom: 1,
          borderColor: "divider",
        }}
      >
        <Typography variant="h6" sx={{ fontSize: "1.1rem", fontWeight: 600 }}>
          {t("wizard.scanQrTitle", "Scan QR Code")}
        </Typography>
        <IconButton
          onClick={() => {
            stopCamera();
            onCancel();
          }}
          size="small"
        >
          <CloseIcon />
        </IconButton>
      </Box>

      {/* Body */}
      <Box
        sx={{
          flex: 1,
          display: "flex",
          flexDirection: "column",
          alignItems: "center",
          justifyContent: "center",
          px: 3,
          gap: 3,
        }}
      >
        {mode === "choose" && (
          <>
            <QrCodeScannerIcon
              sx={{
                fontSize: 64,
                color: alpha(theme.palette.primary.main, 0.6),
              }}
            />
            <Typography
              variant="body2"
              sx={{ color: "text.secondary", textAlign: "center", mb: 1 }}
            >
              {t(
                "wizard.scanQrDescription",
                "Scan a QR code from your admin panel, or paste the provisioning link."
              )}
            </Typography>

            <Button
              variant="contained"
              startIcon={<CameraAltIcon />}
              onClick={startCamera}
              sx={{ borderRadius: "20px", px: 4, py: 1.2, width: "100%" }}
            >
              {t("wizard.openCamera", "Open Camera")}
            </Button>

            <Button
              variant="outlined"
              startIcon={<ContentPasteIcon />}
              onClick={() => setMode("paste")}
              sx={{ borderRadius: "20px", px: 4, py: 1.2, width: "100%" }}
            >
              {t("wizard.pasteLink", "Paste Link")}
            </Button>

            <Button
              variant="text"
              onClick={handlePasteFromClipboard}
              sx={{
                borderRadius: "20px",
                color: "text.secondary",
                fontSize: "0.8rem",
              }}
            >
              {t("wizard.pasteFromClipboard", "Paste from clipboard")}
            </Button>
          </>
        )}

        {mode === "camera" && (
          <>
            <Box
              sx={{
                width: "100%",
                maxWidth: 320,
                aspectRatio: "1",
                borderRadius: "16px",
                overflow: "hidden",
                border: 2,
                borderColor: "primary.main",
                position: "relative",
              }}
            >
              <div
                id={containerId}
                style={{ width: "100%", height: "100%" }}
              />
              {!cameraActive && (
                <Box
                  sx={{
                    position: "absolute",
                    inset: 0,
                    display: "flex",
                    alignItems: "center",
                    justifyContent: "center",
                    bgcolor: alpha(theme.palette.background.default, 0.8),
                  }}
                >
                  <CircularProgress size={32} />
                </Box>
              )}
            </Box>

            <Typography
              variant="body2"
              sx={{ color: "text.secondary", textAlign: "center" }}
            >
              {t(
                "wizard.pointAtQr",
                "Point your camera at the QR code"
              )}
            </Typography>

            <Button
              variant="text"
              onClick={() => {
                stopCamera();
                setMode("choose");
              }}
              sx={{ borderRadius: "20px", color: "text.secondary" }}
            >
              {t("common.cancel", "Cancel")}
            </Button>
          </>
        )}

        {mode === "paste" && (
          <>
            <QrCodeScannerIcon
              sx={{
                fontSize: 48,
                color: alpha(theme.palette.primary.main, 0.4),
              }}
            />

            <TextField
              multiline
              rows={3}
              fullWidth
              value={pasteValue}
              onChange={(e) => {
                setPasteValue(e.target.value);
                setError(null);
              }}
              placeholder="aria://provision?server=...&user=..."
              sx={{
                "& .MuiOutlinedInput-root": {
                  borderRadius: "14px",
                  fontFamily: "monospace",
                  fontSize: "0.8rem",
                },
              }}
            />

            <Box sx={{ display: "flex", gap: 1.5, width: "100%" }}>
              <Button
                variant="outlined"
                onClick={handlePasteFromClipboard}
                startIcon={<ContentPasteIcon />}
                sx={{ borderRadius: "20px", flex: 1 }}
              >
                {t("wizard.paste", "Paste")}
              </Button>
              <Button
                variant="contained"
                onClick={handlePaste}
                startIcon={<CheckCircleOutlineIcon />}
                disabled={!pasteValue.trim()}
                sx={{ borderRadius: "20px", flex: 1 }}
              >
                {t("wizard.connect", "Connect")}
              </Button>
            </Box>

            <Button
              variant="text"
              onClick={() => {
                setMode("choose");
                setPasteValue("");
                setError(null);
              }}
              sx={{ borderRadius: "20px", color: "text.secondary" }}
            >
              {t("common.cancel", "Cancel")}
            </Button>
          </>
        )}

        {error && (
          <Typography
            variant="body2"
            sx={{ color: "error.main", fontSize: "0.8rem", textAlign: "center" }}
          >
            {error}
          </Typography>
        )}
      </Box>
    </Box>
  );
}
