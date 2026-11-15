-- Avertissement d'une transcription finie (outil Transcription) :
-- `TranscriptionWarningCode` ('partial' : lecture du fichier arrêtée avant sa
-- fin, seul le début est transcrit) ; NULL : aucun.

ALTER TABLE transcriptions ADD COLUMN warning TEXT;
