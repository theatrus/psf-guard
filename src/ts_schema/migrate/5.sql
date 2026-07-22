/*
*/

ALTER TABLE exposuretemplate ADD COLUMN defaultexposure REAL DEFAULT 60;

PRAGMA user_version = 5;
