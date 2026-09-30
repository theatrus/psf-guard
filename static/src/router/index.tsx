import { createHashRouter, RouterProvider } from 'react-router-dom';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import App from '../App';
import MainView from '../components/MainView';
import Overview from '../components/Overview';
import SequenceView from '../components/SequenceView';
import SkyPage from '../components/sky/SkyPage';
import PlanPage from '../components/director/PlanPage';
import LegacyPlanningRedirect from '../components/director/LegacyPlanningRedirect';

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
      { path: 'plan', element: <PlanPage /> },
      // Old Planning links: sent on to their plan or to the Library.
      { path: 'director', element: <LegacyPlanningRedirect /> },
      {
        index: true,
        element: <Overview />
      },
      {
        path: "overview",
        element: <Overview />
      },
      {
        path: "grid",
        element: <MainView />
      },
      {
        path: "detail/:imageId",
        element: <MainView />
      },
      {
        path: "compare/:leftImageId/:rightImageId",
        element: <MainView />
      },
      {
        path: "sequence",
        element: <SequenceView />
      },
      {
        path: "sky",
        element: <SkyPage />
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
