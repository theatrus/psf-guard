/*
*/

ALTER TABLE acquiredimage ADD COLUMN rejectreason TEXT;

PRAGMA user_version = 3;
