import { Alert, Snackbar } from "@mui/material";
import { useTranslation } from "react-i18next";
import { useToastStore } from "../../stores/toastStore";

/** The app-wide toast, fed by `useToastStore`. Mounted once, in App. */
export function AppToast() {
  const { t } = useTranslation();
  const current = useToastStore((s) => s.current);
  const dismiss = useToastStore((s) => s.dismiss);

  return (
    <Snackbar
      key={current?.id}
      open={current !== null}
      autoHideDuration={6000}
      onClose={(_, reason) => {
        // A click elsewhere is not a dismissal: the user may be reaching for
        // the very control that failed.
        if (reason !== "clickaway") dismiss();
      }}
      anchorOrigin={{ vertical: "bottom", horizontal: "center" }}
    >
      <Alert
        onClose={dismiss}
        severity={current?.severity ?? "info"}
        variant="filled"
        closeText={t("common.close")}
        sx={{ width: "100%", alignItems: "center" }}
      >
        {current?.message}
      </Alert>
    </Snackbar>
  );
}
