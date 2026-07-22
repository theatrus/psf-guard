/*
*/

ALTER TABLE profilepreference ADD COLUMN enableSlewCenter INTEGER DEFAULT 1;

PRAGMA user_version = 18;
