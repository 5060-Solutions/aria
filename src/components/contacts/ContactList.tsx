import { useState, useMemo, useCallback } from "react";
import {
  Box,
  List,
  ListItemButton,
  ListItemAvatar,
  ListItemText,
  Avatar,
  Typography,
  IconButton,
  Fab,
  alpha,
  useTheme,
  TextField,
  InputAdornment,
  Dialog,
  DialogTitle,
  DialogContent,
  DialogActions,
  Button,
  Chip,
  Tooltip,
} from "@mui/material";
import CallIcon from "@mui/icons-material/Call";
import PersonAddIcon from "@mui/icons-material/PersonAdd";
import StarIcon from "@mui/icons-material/Star";
import StarBorderIcon from "@mui/icons-material/StarBorder";
import SearchIcon from "@mui/icons-material/Search";
import DeleteOutlineIcon from "@mui/icons-material/DeleteOutline";
import CloseIcon from "@mui/icons-material/Close";
import { useTranslation } from "react-i18next";
import { useAppStore } from "../../stores/appStore";
import type { Contact } from "../../types/sip";

const inputSx = {
  "& .MuiOutlinedInput-root": {
    borderRadius: "14px",
  },
};

function getInitials(name: string): string {
  const parts = name.trim().split(/\s+/);
  if (parts.length >= 2) {
    return (parts[0][0] + parts[1][0]).toUpperCase();
  }
  return name.substring(0, 2).toUpperCase();
}

function avatarColor(name: string): string {
  const colors = [
    "#5C6BC0",
    "#26A69A",
    "#EF5350",
    "#AB47BC",
    "#42A5F5",
    "#66BB6A",
    "#FFA726",
    "#EC407A",
    "#78909C",
    "#8D6E63",
  ];
  let hash = 0;
  for (let i = 0; i < name.length; i++) {
    hash = name.charCodeAt(i) + ((hash << 5) - hash);
  }
  return colors[Math.abs(hash) % colors.length];
}

// --- Add/Edit Contact Dialog ---

interface ContactFormData {
  name: string;
  uri: string;
  phone: string;
  favorite: boolean;
}

function ContactDialog({
  open,
  onClose,
  onSave,
  onDelete,
  initial,
  isEdit,
}: {
  open: boolean;
  onClose: () => void;
  onSave: (data: ContactFormData) => void;
  onDelete?: () => void;
  initial?: ContactFormData;
  isEdit?: boolean;
}) {
  const { t } = useTranslation();
  const [form, setForm] = useState<ContactFormData>(
    initial ?? { name: "", uri: "", phone: "", favorite: false },
  );

  const update = (field: keyof ContactFormData, value: string | boolean) =>
    setForm((f) => ({ ...f, [field]: value }));

  const canSave = form.name.trim() && (form.uri.trim() || form.phone.trim());

  const handleSave = () => {
    if (!canSave) return;
    // Auto-format URI if only a number/extension is entered
    let uri = form.uri.trim();
    if (!uri && form.phone.trim()) {
      uri = `sip:${form.phone.trim()}`;
    }
    if (uri && !uri.startsWith("sip:")) {
      uri = `sip:${uri}`;
    }
    onSave({ ...form, uri });
    onClose();
  };

  return (
    <Dialog
      open={open}
      onClose={onClose}
      maxWidth="xs"
      fullWidth
      PaperProps={{
        sx: { borderRadius: "20px" },
      }}
    >
      <DialogTitle
        sx={{
          display: "flex",
          alignItems: "center",
          justifyContent: "space-between",
          pb: 1,
        }}
      >
        <Typography variant="h6" sx={{ fontWeight: 500, fontSize: "1rem" }}>
          {isEdit ? t("contacts.editContact") : t("contacts.newContact")}
        </Typography>
        <IconButton size="small" onClick={onClose}>
          <CloseIcon fontSize="small" />
        </IconButton>
      </DialogTitle>
      <DialogContent sx={{ display: "flex", flexDirection: "column", gap: 2, pt: 1 }}>
        <TextField
          label={t("contacts.name")}
          size="small"
          value={form.name}
          onChange={(e) => update("name", e.target.value)}
          placeholder={t("contacts.namePlaceholder")}
          autoFocus
          sx={inputSx}
        />
        <TextField
          label={t("contacts.sipUri")}
          size="small"
          value={form.uri}
          onChange={(e) => update("uri", e.target.value)}
          placeholder={t("contacts.sipUriPlaceholder")}
          sx={inputSx}
        />
        <TextField
          label={t("contacts.phoneNumber")}
          size="small"
          value={form.phone}
          onChange={(e) => update("phone", e.target.value)}
          placeholder={t("contacts.optional")}
          sx={inputSx}
        />
        <Box sx={{ display: "flex", alignItems: "center", gap: 1 }}>
          <IconButton
            size="small"
            onClick={() => update("favorite", !form.favorite)}
            sx={{ color: form.favorite ? "warning.main" : "text.secondary" }}
          >
            {form.favorite ? (
              <StarIcon fontSize="small" />
            ) : (
              <StarBorderIcon fontSize="small" />
            )}
          </IconButton>
          <Typography variant="body2" sx={{ color: "text.secondary", fontSize: "0.85rem" }}>
            {form.favorite ? t("contacts.favorite") : t("contacts.addToFavorites")}
          </Typography>
        </Box>
      </DialogContent>
      <DialogActions sx={{ px: 3, pb: 2.5, gap: 1 }}>
        {isEdit && onDelete && (
          <Button
            onClick={() => {
              onDelete();
              onClose();
            }}
            color="error"
            size="small"
            startIcon={<DeleteOutlineIcon />}
            sx={{ mr: "auto", textTransform: "none", borderRadius: "12px" }}
          >
            {t("common.delete")}
          </Button>
        )}
        <Button
          onClick={onClose}
          size="small"
          sx={{ textTransform: "none", borderRadius: "12px" }}
        >
          {t("common.cancel")}
        </Button>
        <Button
          variant="contained"
          onClick={handleSave}
          disabled={!canSave}
          size="small"
          sx={{ textTransform: "none", borderRadius: "12px", px: 3 }}
        >
          {isEdit ? t("common.save") : t("common.add")}
        </Button>
      </DialogActions>
    </Dialog>
  );
}

// --- Contact Item ---

function ContactItem({
  contact,
  onCall,
  onEdit,
  onToggleFavorite,
}: {
  contact: Contact;
  onCall: () => void;
  onEdit: () => void;
  onToggleFavorite: () => void;
}) {
  const { t } = useTranslation();
  const theme = useTheme();
  const bgColor = avatarColor(contact.name);
  const displayUri = contact.uri.replace(/^sip:/, "");
  const secondary = contact.phone
    ? `${displayUri} | ${contact.phone}`
    : displayUri;

  return (
    <ListItemButton
      onClick={onEdit}
      sx={{ borderRadius: "16px", mb: 0.5, py: 1 }}
    >
      <ListItemAvatar>
        <Avatar
          sx={{
            width: 40,
            height: 40,
            bgcolor: alpha(bgColor, 0.15),
            color: bgColor,
            fontSize: "0.85rem",
            fontWeight: 600,
          }}
        >
          {getInitials(contact.name)}
        </Avatar>
      </ListItemAvatar>
      <ListItemText
        primary={
          <Box sx={{ display: "flex", alignItems: "center", gap: 0.5 }}>
            {contact.name}
            {contact.favorite && (
              <StarIcon sx={{ fontSize: 14, color: "warning.main" }} />
            )}
          </Box>
        }
        secondary={secondary}
        primaryTypographyProps={{ fontSize: "0.9rem", fontWeight: 500 }}
        secondaryTypographyProps={{
          fontSize: "0.75rem",
          sx: {
            whiteSpace: "nowrap",
            overflow: "hidden",
            textOverflow: "ellipsis",
          },
        }}
      />
      <Box sx={{ display: "flex", gap: 0.25, ml: 0.5 }}>
        <Tooltip title={contact.favorite ? t("contacts.removeFavorite") : t("contacts.addFavorite")}>
          <IconButton
            size="small"
            onClick={(e) => {
              e.stopPropagation();
              onToggleFavorite();
            }}
            sx={{
              color: contact.favorite ? "warning.main" : alpha(theme.palette.text.secondary, 0.3),
              borderRadius: "10px",
              "&:hover": {
                color: "warning.main",
              },
            }}
          >
            {contact.favorite ? (
              <StarIcon sx={{ fontSize: 16 }} />
            ) : (
              <StarBorderIcon sx={{ fontSize: 16 }} />
            )}
          </IconButton>
        </Tooltip>
        <IconButton
          size="small"
          onClick={(e) => {
            e.stopPropagation();
            onCall();
          }}
          sx={{ color: "primary.main", borderRadius: "10px" }}
        >
          <CallIcon sx={{ fontSize: 18 }} />
        </IconButton>
      </Box>
    </ListItemButton>
  );
}

// --- Alphabetical Section ---

function AlphaSection({ letter, children }: { letter: string; children: React.ReactNode }) {
  const theme = useTheme();
  return (
    <Box>
      <Box
        sx={{
          px: 2,
          py: 0.5,
          position: "sticky",
          top: 0,
          zIndex: 1,
          bgcolor: alpha(theme.palette.background.default, 0.9),
          backdropFilter: "blur(8px)",
        }}
      >
        <Typography
          variant="caption"
          sx={{
            color: "primary.main",
            fontWeight: 700,
            fontSize: "0.7rem",
          }}
        >
          {letter}
        </Typography>
      </Box>
      {children}
    </Box>
  );
}

// --- Main Component ---

export function ContactList() {
  const { t } = useTranslation();
  const contacts = useAppStore((s) => s.contacts);
  const addContact = useAppStore((s) => s.addContact);
  const removeContact = useAppStore((s) => s.removeContact);
  const setDialInput = useAppStore((s) => s.setDialInput);
  const setCurrentView = useAppStore((s) => s.setCurrentView);
  const accounts = useAppStore((s) => s.accounts);
  const activeAccountId = useAppStore((s) => s.activeAccountId);
  const setActiveCall = useAppStore((s) => s.setActiveCall);
  const theme = useTheme();

  const activeAccount = accounts.find((a) => a.id === activeAccountId);

  const [search, setSearch] = useState("");
  const [dialogOpen, setDialogOpen] = useState(false);
  const [editingContact, setEditingContact] = useState<Contact | null>(null);

  const handleCall = useCallback(
    async (uri: string, name?: string) => {
      if (!activeAccountId || !activeAccount) {
        const number = uri.replace(/^sip:/, "").split("@")[0];
        setDialInput(number);
        setCurrentView("dialer");
        return;
      }

      const number = uri.replace(/^sip:/, "").split("@")[0];
      const fullUri = uri.startsWith("sip:") ? uri : `sip:${number}@${activeAccount.domain}`;

      setActiveCall({
        id: crypto.randomUUID(),
        accountId: activeAccountId,
        remoteUri: fullUri,
        remoteName: name || number,
        state: "dialing",
        direction: "outbound",
        startTime: Date.now(),
        muted: false,
        held: false,
        recording: false,
      });

      try {
        const { sipMakeCall } = await import("../../hooks/useSip");
        const callId = await sipMakeCall(fullUri);
        setActiveCall({
          id: callId,
          accountId: activeAccountId,
          remoteUri: fullUri,
          remoteName: name || number,
          state: "dialing",
          direction: "outbound",
          startTime: Date.now(),
          muted: false,
          held: false,
          recording: false,
        });
      } catch {
        setActiveCall(null);
      }
    },
    [activeAccountId, activeAccount, setDialInput, setCurrentView, setActiveCall],
  );

  const handleAddContact = useCallback(
    (data: ContactFormData) => {
      const newContact: Contact = {
        id: crypto.randomUUID(),
        name: data.name.trim(),
        uri: data.uri,
        phone: data.phone.trim() || undefined,
        favorite: data.favorite,
        source: "local",
      };
      addContact(newContact);
    },
    [addContact],
  );

  const handleEditContact = useCallback(
    (data: ContactFormData) => {
      if (!editingContact) return;
      // Remove old, add updated
      removeContact(editingContact.id);
      addContact({
        ...editingContact,
        name: data.name.trim(),
        uri: data.uri,
        phone: data.phone.trim() || undefined,
        favorite: data.favorite,
      });
      setEditingContact(null);
    },
    [editingContact, removeContact, addContact],
  );

  const handleToggleFavorite = useCallback(
    (contact: Contact) => {
      removeContact(contact.id);
      addContact({ ...contact, favorite: !contact.favorite });
    },
    [removeContact, addContact],
  );

  // Filter and group
  const filtered = useMemo(() => {
    const q = search.toLowerCase().trim();
    if (!q) return contacts;
    return contacts.filter(
      (c) =>
        c.name.toLowerCase().includes(q) ||
        c.uri.toLowerCase().includes(q) ||
        (c.phone && c.phone.includes(q)),
    );
  }, [contacts, search]);

  const { favorites, grouped } = useMemo(() => {
    const favs = filtered
      .filter((c) => c.favorite)
      .sort((a, b) => a.name.localeCompare(b.name));

    const nonFavs = filtered
      .filter((c) => !c.favorite)
      .sort((a, b) => a.name.localeCompare(b.name));

    const groups: Record<string, Contact[]> = {};
    for (const c of nonFavs) {
      const letter = c.name[0]?.toUpperCase() || "#";
      if (!groups[letter]) groups[letter] = [];
      groups[letter].push(c);
    }

    return { favorites: favs, grouped: groups };
  }, [filtered]);

  const sortedLetters = Object.keys(grouped).sort();

  return (
    <Box
      sx={{
        height: "100%",
        display: "flex",
        flexDirection: "column",
        position: "relative",
      }}
    >
      {/* Header */}
      <Box sx={{ px: 2.5, pt: 2.5, pb: 1 }}>
        <Box
          sx={{
            display: "flex",
            alignItems: "center",
            justifyContent: "space-between",
            mb: 1.5,
          }}
        >
          <Typography variant="h5" sx={{ fontWeight: 500 }}>
            {t("contacts.title")}
          </Typography>
          <Chip
            label={contacts.length}
            size="small"
            sx={{
              height: 22,
              fontSize: "0.7rem",
              fontWeight: 600,
              bgcolor: alpha(theme.palette.primary.main, 0.08),
              color: "primary.main",
            }}
          />
        </Box>

        {/* Search */}
        <TextField
          size="small"
          placeholder={t("contacts.search")}
          value={search}
          onChange={(e) => setSearch(e.target.value)}
          fullWidth
          slotProps={{
            input: {
              startAdornment: (
                <InputAdornment position="start">
                  <SearchIcon sx={{ fontSize: 18, color: "text.secondary" }} />
                </InputAdornment>
              ),
              endAdornment: search ? (
                <InputAdornment position="end">
                  <IconButton size="small" onClick={() => setSearch("")}>
                    <CloseIcon sx={{ fontSize: 14 }} />
                  </IconButton>
                </InputAdornment>
              ) : null,
            },
          }}
          sx={{
            "& .MuiOutlinedInput-root": {
              borderRadius: "14px",
              bgcolor: alpha(theme.palette.text.primary, 0.03),
            },
          }}
        />
      </Box>

      {/* Contact list */}
      {filtered.length === 0 ? (
        <Box
          sx={{
            flex: 1,
            display: "flex",
            flexDirection: "column",
            alignItems: "center",
            justifyContent: "center",
            color: "text.secondary",
            gap: 1,
          }}
        >
          {contacts.length === 0 ? (
            <>
              <PersonAddIcon sx={{ fontSize: 48, opacity: 0.3 }} />
              <Typography variant="body2">{t("contacts.noContacts")}</Typography>
              <Typography variant="caption" sx={{ opacity: 0.6 }}>
                {t("contacts.tapToAdd")}
              </Typography>
            </>
          ) : (
            <>
              <SearchIcon sx={{ fontSize: 36, opacity: 0.3 }} />
              <Typography variant="body2">
                {t("contacts.noResults", { search })}
              </Typography>
            </>
          )}
        </Box>
      ) : (
        <List
          sx={{
            flex: 1,
            overflow: "auto",
            px: 1,
            "&::-webkit-scrollbar": { width: 4 },
            "&::-webkit-scrollbar-thumb": {
              bgcolor: alpha(theme.palette.text.primary, 0.08),
              borderRadius: 2,
            },
          }}
        >
          {/* Favorites section */}
          {favorites.length > 0 && (
            <>
              <Box sx={{ px: 1.5, py: 0.75 }}>
                <Typography
                  variant="caption"
                  sx={{
                    color: "warning.main",
                    fontWeight: 600,
                    fontSize: "0.68rem",
                    letterSpacing: "0.05em",
                    display: "flex",
                    alignItems: "center",
                    gap: 0.5,
                  }}
                >
                  <StarIcon sx={{ fontSize: 12 }} />
                  {t("contacts.favorites")}
                </Typography>
              </Box>
              {favorites.map((contact) => (
                <ContactItem
                  key={contact.id}
                  contact={contact}
                  onCall={() => handleCall(contact.uri, contact.name)}
                  onEdit={() => setEditingContact(contact)}
                  onToggleFavorite={() => handleToggleFavorite(contact)}
                />
              ))}
            </>
          )}

          {/* Alphabetical sections */}
          {sortedLetters.map((letter) => (
            <AlphaSection key={letter} letter={letter}>
              {grouped[letter].map((contact) => (
                <ContactItem
                  key={contact.id}
                  contact={contact}
                  onCall={() => handleCall(contact.uri, contact.name)}
                  onEdit={() => setEditingContact(contact)}
                  onToggleFavorite={() => handleToggleFavorite(contact)}
                />
              ))}
            </AlphaSection>
          ))}
        </List>
      )}

      {/* FAB */}
      <Fab
        color="primary"
        size="medium"
        onClick={() => setDialogOpen(true)}
        sx={{
          position: "absolute",
          bottom: 16,
          right: 16,
          borderRadius: "16px",
          boxShadow: `0 4px 16px ${alpha(theme.palette.primary.main, 0.3)}`,
        }}
      >
        <PersonAddIcon />
      </Fab>

      {/* Add dialog */}
      <ContactDialog
        open={dialogOpen}
        onClose={() => setDialogOpen(false)}
        onSave={handleAddContact}
      />

      {/* Edit dialog */}
      {editingContact && (
        <ContactDialog
          open={!!editingContact}
          onClose={() => setEditingContact(null)}
          onSave={handleEditContact}
          onDelete={() => {
            removeContact(editingContact.id);
            setEditingContact(null);
          }}
          initial={{
            name: editingContact.name,
            uri: editingContact.uri,
            phone: editingContact.phone ?? "",
            favorite: editingContact.favorite,
          }}
          isEdit
        />
      )}
    </Box>
  );
}
