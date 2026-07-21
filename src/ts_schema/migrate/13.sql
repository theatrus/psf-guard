/*
*/

ALTER TABLE profilepreference ADD COLUMN enableDeleteAcquiredImagesWithTarget INTEGER DEFAULT 1;

PRAGMA user_version = 13;
