/*
*/

ALTER TABLE exposuretemplate ADD COLUMN moonrelaxscale REAL DEFAULT 0;
ALTER TABLE exposuretemplate ADD COLUMN moonrelaxmaxaltitude REAL DEFAULT 5;
ALTER TABLE exposuretemplate ADD COLUMN moonrelaxminaltitude REAL DEFAULT -15;

PRAGMA user_version = 14;
