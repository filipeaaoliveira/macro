-- Validate the review findings' checks added NOT VALID in 20261120030000, in
-- its own transaction, so the lock that migration took is not held through
-- the scan. It takes a SHARE UPDATE EXCLUSIVE lock: reads and writes go on
-- meanwhile. The columns are new and every kept kind is still allowed, so
-- every row passes.
SET LOCAL lock_timeout = '5s';

ALTER TABLE factory_review_finding VALIDATE CONSTRAINT factory_review_finding_kind_check;
ALTER TABLE factory_review_finding VALIDATE CONSTRAINT factory_review_finding_dispute;
