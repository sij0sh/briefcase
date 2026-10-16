"""In-memory executable publication specification, not durable product code."""


class LifecycleOracle:
    def __init__(self):
        self.watermark = 0
        self.batches = {}
        self.assertions = {}
        self.generations = {}
        self.retracted = set()
        self.publications = {}
        self.history = {}

    def eligible(self, assertion_id, visiting=None):
        visiting = set() if visiting is None else visiting
        if assertion_id in visiting or assertion_id in self.retracted:
            return False
        if assertion_id not in self.assertions:
            return False
        return all(self.eligible(x, visiting | {assertion_id})
                   for x in self.assertions[assertion_id])

    def apply(self, action):
        operation = action["operation"]
        if operation in {"interrupt_prepare", "projection_failure", "restart"}:
            return {"status": "no_publication", "watermark": self.watermark}
        if operation == "commit":
            batch_id, digest = action["batch_id"], action["digest"]
            if batch_id in self.batches:
                prior_digest, prior_watermark = self.batches[batch_id]
                if digest != prior_digest:
                    return {"status": "conflicting_replay", "watermark": self.watermark}
                return {"status": "replayed", "watermark": prior_watermark}
            for row in action["assertions"]:
                if row["id"] in self.assertions and self.assertions[row["id"]] != row["supports"]:
                    return {"status": "assertion_conflict", "watermark": self.watermark}
            self.watermark += 1
            self.batches[batch_id] = (digest, self.watermark)
            for row in action["assertions"]:
                self.assertions[row["id"]] = list(row["supports"])
            return {"status": "committed", "watermark": self.watermark}
        if operation == "publish":
            scope = action["scope"]
            if action["generation"] <= self.generations.get(scope, 0):
                return {"status": "stale_publication", "watermark": self.watermark}
            if action["input_watermark"] > self.watermark:
                return {"status": "uncommitted_input", "watermark": self.watermark}
            if not action["complete"]:
                return {"status": "incomplete_view", "watermark": self.watermark}
            if any(x not in self.assertions for x in action["assertion_ids"]):
                return {"status": "unknown_input", "watermark": self.watermark}
            self.generations[scope] = action["generation"]
            self.publications[scope] = list(action["assertion_ids"])
            self.history[(scope, action["generation"])] = list(action["assertion_ids"])
            return {"status": "published", "watermark": self.watermark}
        if operation == "retract":
            if action["assertion_id"] not in self.assertions:
                return {"status": "unknown_input", "watermark": self.watermark}
            self.retracted.add(action["assertion_id"])
            self.watermark += 1
            return {"status": "retracted", "watermark": self.watermark}
        if operation == "query":
            ids = self.publications.get(action["scope"], [])
            if action["view"] == "historical":
                if "generation" in action:
                    ids = self.history.get((action["scope"], action["generation"]), [])
                return {"status": "historical", "ids": sorted(ids),
                        "ineligible_ids": sorted(x for x in ids if not self.eligible(x))}
            return {"status": "current", "ids": sorted(x for x in ids if self.eligible(x))}
        raise ValueError(f"Unsupported oracle operation: {operation}")
