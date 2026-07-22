/*
*/

ALTER TABLE profilepreference ADD COLUMN enableSmartPlanWindow INTEGER DEFAULT 1;

ALTER TABLE profilepreference ADD COLUMN enableSynchronization INTEGER DEFAULT 0;
ALTER TABLE profilepreference ADD COLUMN syncWaitTimeout INTEGER DEFAULT 300;
ALTER TABLE profilepreference ADD COLUMN syncActionTimeout INTEGER DEFAULT 300;
ALTER TABLE profilepreference ADD COLUMN syncSolveRotateTimeout INTEGER DEFAULT 300;

ALTER TABLE profilepreference ADD COLUMN enableMoveRejected INTEGER DEFAULT 0;
ALTER TABLE profilepreference ADD COLUMN enableGradeFWHM INTEGER DEFAULT 0;
ALTER TABLE profilepreference ADD COLUMN enableGradeEccentricity INTEGER DEFAULT 0;
ALTER TABLE profilepreference ADD COLUMN fwhmSigmaFactor INTEGER DEFAULT 4;
ALTER TABLE profilepreference ADD COLUMN eccentricitySigmaFactor INTEGER DEFAULT 4;

ALTER TABLE acquiredimage ADD COLUMN profileId TEXT;

PRAGMA user_version = 10;
