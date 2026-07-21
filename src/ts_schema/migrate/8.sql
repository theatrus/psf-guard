/*
*/

ALTER TABLE project ADD COLUMN isMosaic INTEGER NOT NULL DEFAULT 0;
UPDATE project SET isMosaic = 0;

PRAGMA user_version = 8;
