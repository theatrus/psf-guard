/*
*/

ALTER TABLE profilepreference ADD COLUMN logLevel INTEGER DEFAULT 3;

PRAGMA user_version = 19;
