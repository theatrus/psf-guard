import { useMemo } from 'react';
import { type Image, GradingStatus } from '../api/types';

interface StatsDashboardProps {
  images: Image[];
  className?: string;
}

interface TargetStats {
  targetId: number;
  targetName: string;
  total: number;
  accepted: number;
  rejected: number;
  pending: number;
  filters: Record<string, FilterStats>;
}

interface FilterStats {
  total: number;
  accepted: number;
  rejected: number;
  pending: number;
}

export default function StatsDashboard({ images, className = '' }: StatsDashboardProps) {
  const stats = useMemo(() => {
    const targetMap = new Map<number, TargetStats>();

    images.forEach(image => {
      const targetId = image.target_id;
      
      if (!targetMap.has(targetId)) {
        targetMap.set(targetId, {
          targetId,
          targetName: image.target_name,
          total: 0,
          accepted: 0,
          rejected: 0,
          pending: 0,
          filters: {},
        });
      }

      const targetStats = targetMap.get(targetId)!;
      targetStats.total++;

      // Update status counts
      switch (image.grading_status) {
        case GradingStatus.Accepted:
          targetStats.accepted++;
          break;
        case GradingStatus.Rejected:
          targetStats.rejected++;
          break;
        case GradingStatus.Pending:
          targetStats.pending++;
          break;
      }

      // Update filter-specific stats
      const filterName = image.filter_name || 'No Filter';
      if (!targetStats.filters[filterName]) {
        targetStats.filters[filterName] = {
          total: 0,
          accepted: 0,
          rejected: 0,
          pending: 0,
        };
      }

      const filterStats = targetStats.filters[filterName];
      filterStats.total++;

      switch (image.grading_status) {
        case GradingStatus.Accepted:
          filterStats.accepted++;
          break;
        case GradingStatus.Rejected:
          filterStats.rejected++;
          break;
        case GradingStatus.Pending:
          filterStats.pending++;
          break;
      }
    });

    return Array.from(targetMap.values()).sort((a, b) => 
      a.targetName.localeCompare(b.targetName)
    );
  }, [images]);

  const totalStats = useMemo(() => {
    return stats.reduce(
      (acc, target) => ({
        total: acc.total + target.total,
        accepted: acc.accepted + target.accepted,
        rejected: acc.rejected + target.rejected,
        pending: acc.pending + target.pending,
      }),
      { total: 0, accepted: 0, rejected: 0, pending: 0 }
    );
  }, [stats]);

  const formatPercentage = (value: number, total: number) => {
    if (total === 0) return '0%';
    return `${((value / total) * 100).toFixed(1)}%`;
  };

  return (
    <div className={`stats-dashboard ${className}`}>
      <h2>Grading Statistics</h2>
      
      {/* Overall Summary */}
      <div className="stats-summary">
        <h3>Overall Summary</h3>
        <div className="stats-grid">
          <div className="stat-card">
            <div className="stat-value">{totalStats.total}</div>
            <div className="stat-label">Total Images</div>
          </div>
          <div className="stat-card accepted">
            <div className="stat-value">{totalStats.accepted}</div>
            <div className="stat-label">Accepted</div>
            <div className="stat-percentage">{formatPercentage(totalStats.accepted, totalStats.total)}</div>
          </div>
          <div className="stat-card rejected">
            <div className="stat-value">{totalStats.rejected}</div>
            <div className="stat-label">Rejected</div>
            <div className="stat-percentage">{formatPercentage(totalStats.rejected, totalStats.total)}</div>
          </div>
          <div className="stat-card pending">
            <div className="stat-value">{totalStats.pending}</div>
            <div className="stat-label">Pending</div>
            <div className="stat-percentage">{formatPercentage(totalStats.pending, totalStats.total)}</div>
          </div>
        </div>
      </div>

      {/* Target Breakdown */}
      <div className="target-stats">
        <h3>Target Breakdown</h3>
        {stats.map(target => (
          <div key={target.targetId} className="target-section">
            <h4>{target.targetName}</h4>
            
            <div className="target-summary">
              <span className="stat-item">
                <strong>Total:</strong> {target.total}
              </span>
              <span className="stat-item accepted">
                <strong>Accepted:</strong> {target.accepted} ({formatPercentage(target.accepted, target.total)})
              </span>
              <span className="stat-item rejected">
                <strong>Rejected:</strong> {target.rejected} ({formatPercentage(target.rejected, target.total)})
              </span>
              <span className="stat-item pending">
                <strong>Pending:</strong> {target.pending} ({formatPercentage(target.pending, target.total)})
              </span>
            </div>

            {/* Filter breakdown */}
            <div className="filter-stats">
              <h5>By Filter</h5>
              <table className="filter-table">
                <thead>
                  <tr>
                    <th>Filter</th>
                    <th>Total</th>
                    <th>Accepted</th>
                    <th>Rejected</th>
                    <th>Pending</th>
                  </tr>
                </thead>
                <tbody>
                  {Object.entries(target.filters).map(([filterName, filterStats]) => (
                    <tr key={filterName}>
                      <td>{filterName}</td>
                      <td>{filterStats.total}</td>
                      <td className="accepted">
                        {filterStats.accepted} ({formatPercentage(filterStats.accepted, filterStats.total)})
                      </td>
                      <td className="rejected">
                        {filterStats.rejected} ({formatPercentage(filterStats.rejected, filterStats.total)})
                      </td>
                      <td className="pending">
                        {filterStats.pending} ({formatPercentage(filterStats.pending, filterStats.total)})
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}