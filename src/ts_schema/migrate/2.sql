/*
*/

CREATE TABLE IF NOT EXISTS `profilepreference` (
	`Id`			INTEGER NOT NULL,
	`profileId`		TEXT NOT NULL,
	`enableGradeRMS`	INTEGER,
	`enableGradeStars`	INTEGER,
	`enableGradeHFR`	INTEGER,
	`maxGradingSampleSize`		INTEGER,
	`rmsPixelThreshold`			REAL,
	`detectedStarsSigmaFactor`	REAL,
	`hfrSigmaFactor`			REAL,
	PRIMARY KEY(`id`)
);

PRAGMA user_version = 2;
