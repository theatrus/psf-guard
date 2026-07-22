/*
*/

ALTER TABLE acquiredimage ADD COLUMN guid TEXT;
ALTER TABLE exposureplan ADD COLUMN guid TEXT;
ALTER TABLE exposuretemplate ADD COLUMN guid TEXT;
ALTER TABLE profilepreference ADD COLUMN guid TEXT;
ALTER TABLE project ADD COLUMN guid TEXT;
ALTER TABLE target ADD COLUMN guid TEXT;

ALTER TABLE profilepreference ADD COLUMN enableProfileTargetCompletionReset INTEGER DEFAULT 0;

PRAGMA user_version = 22;
