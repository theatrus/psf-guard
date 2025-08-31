import { createHashRouter, RouterProvider, Navigate } from 'react-router-dom';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import App from '../App';
import GridView from '../components/GridView';
import DetailView from '../components/DetailView';
import ComparisonView from '../components/ComparisonView';

// Create React Query client
const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      staleTime: 30000,
      refetchInterval: 30000,
    },
  },
});

// Create hash router
const router = createHashRouter([
  {
    path: "/",
    element: <App />,
    children: [
      {
        index: true,
        element: <Navigate to="/grid" replace />
      },
      {
        path: "grid",
        element: <GridView />
      },
      {
        path: "detail/:imageId",
        element: <DetailView />
      },
      {
        path: "compare/:leftImageId/:rightImageId",
        element: <ComparisonView />
      }
    ]
  }
]);

export function AppRouter() {
  return (
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>
  );
}