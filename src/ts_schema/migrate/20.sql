/*
*/

ALTER TABLE exposuretemplate ADD COLUMN ditherevery INTEGER DEFAULT -1;

PRAGMA user_version = 20;
