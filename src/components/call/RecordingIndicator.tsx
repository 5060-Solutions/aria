import { Box, keyframes, Typography, alpha } from "@mui/material";
import FiberManualRecordIcon from "@mui/icons-material/FiberManualRecord";
import { useTranslation } from "react-i18next";

const pulse = keyframes`
  0%, 100% {
    opacity: 1;
    transform: scale(1);
  }
  50% {
    opacity: 0.5;
    transform: scale(0.9);
  }
`;

const ripple = keyframes`
  0% {
    transform: scale(1);
    opacity: 0.4;
  }
  100% {
    transform: scale(2.5);
    opacity: 0;
  }
`;

interface RecordingIndicatorProps {
  variant?: "compact" | "full";
  showLabel?: boolean;
}

export function RecordingIndicator({ variant = "compact", showLabel = true }: RecordingIndicatorProps) {
  const { t } = useTranslation();

  if (variant === "compact") {
    return (
      <Box
        sx={{
          display: "inline-flex",
          alignItems: "center",
          gap: 0.5,
          px: 1,
          py: 0.25,
          borderRadius: 2,
          bgcolor: (theme) => alpha(theme.palette.error.main, 0.1),
          border: (theme) => `1px solid ${alpha(theme.palette.error.main, 0.2)}`,
        }}
      >
        <Box sx={{ position: "relative", display: "flex", alignItems: "center" }}>
          <FiberManualRecordIcon
            sx={{
              fontSize: 10,
              color: "error.main",
              animation: `${pulse} 1.5s ease-in-out infinite`,
            }}
          />
        </Box>
        {showLabel && (
          <Typography
            variant="caption"
            sx={{
              fontSize: "0.65rem",
              fontWeight: 600,
              color: "error.main",
              textTransform: "uppercase",
              letterSpacing: "0.05em",
            }}
          >
            {t("call.recording")}
          </Typography>
        )}
      </Box>
    );
  }

  return (
    <Box
      sx={{
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        gap: 1.5,
        px: 2,
        py: 1,
        borderRadius: 3,
        bgcolor: (theme) => alpha(theme.palette.error.main, 0.08),
        border: (theme) => `1px solid ${alpha(theme.palette.error.main, 0.15)}`,
      }}
    >
      <Box sx={{ position: "relative", width: 24, height: 24, display: "flex", alignItems: "center", justifyContent: "center" }}>
        {/* Ripple effect */}
        <Box
          sx={{
            position: "absolute",
            width: 16,
            height: 16,
            borderRadius: "50%",
            bgcolor: "error.main",
            animation: `${ripple} 2s ease-out infinite`,
          }}
        />
        <Box
          sx={{
            position: "absolute",
            width: 16,
            height: 16,
            borderRadius: "50%",
            bgcolor: "error.main",
            animation: `${ripple} 2s ease-out infinite`,
            animationDelay: "0.5s",
          }}
        />
        <Box
          sx={{
            position: "absolute",
            width: 16,
            height: 16,
            borderRadius: "50%",
            bgcolor: "error.main",
            animation: `${ripple} 2s ease-out infinite`,
            animationDelay: "1s",
          }}
        />
        {/* Center dot */}
        <FiberManualRecordIcon
          sx={{
            fontSize: 16,
            color: "error.main",
            position: "relative",
            zIndex: 1,
            animation: `${pulse} 1.5s ease-in-out infinite`,
          }}
        />
      </Box>
      <Box>
        <Typography
          variant="caption"
          sx={{
            fontSize: "0.75rem",
            fontWeight: 600,
            color: "error.main",
            textTransform: "uppercase",
            letterSpacing: "0.05em",
            display: "block",
          }}
        >
          {t("call.recording")}
        </Typography>
        <Typography
          variant="caption"
          sx={{
            fontSize: "0.65rem",
            color: "error.light",
            opacity: 0.8,
          }}
        >
          {t("call.recordingInProgress")}
        </Typography>
      </Box>
    </Box>
  );
}
