/*
*/

ALTER TABLE exposuretemplate ADD COLUMN moondownenabled INTEGER DEFAULT 0;

PRAGMA user_version = 16;
