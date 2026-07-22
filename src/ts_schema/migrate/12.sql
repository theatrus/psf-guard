/*
*/

ALTER TABLE flathistory ADD COLUMN lightSessionId INTEGER NOT NULL DEFAULT 0;
UPDATE flathistory SET lightSessionId = 0;

PRAGMA user_version = 12;
