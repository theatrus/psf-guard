/*
*/

ALTER TABLE profilepreference ADD COLUMN acceptimprovement INTEGER DEFAULT 1;
ALTER TABLE profilepreference ADD COLUMN exposurethrottle REAL DEFAULT 125;
ALTER TABLE profilepreference ADD COLUMN parkonwait INTEGER DEFAULT 0;

PRAGMA user_version = 4;
