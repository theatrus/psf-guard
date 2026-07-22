/*
*/

ALTER TABLE project DROP COLUMN startdate;
ALTER TABLE project DROP COLUMN enddate;

PRAGMA user_version = 1;
